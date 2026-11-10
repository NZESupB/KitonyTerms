package update

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
)

// 在原生 GUI/数据服务初始化前分流；discover 只在已验证的正式 helper 入口调用。
func HandleDesktopCommand(ctx context.Context, args []string, output io.Writer, discover func() (string, error)) (bool, error) {
	if len(args) == 0 || args[0] != "--apply-update" && args[0] != "--relaunch-update" && args[0] != "--elevated-replace" && args[0] != "--restore-update" {
		return HandleCommand(args, output)
	}
	current, err := CurrentBuild()
	if err != nil {
		return true, err
	}
	if current.Channel == "development" {
		return HandleCommand(args, output)
	}
	self, err := os.Executable()
	if err != nil {
		return true, err
	}
	if filepath.Base(self) != updateHelperName() {
		return true, fmt.Errorf("安装辅助命令只能由固定 helper 副本执行")
	}
	if args[0] == "--elevated-replace" || args[0] == "--restore-update" {
		operation, root, digest, parseErr := parseElevationArguments(args)
		if parseErr != nil {
			return true, parseErr
		}
		return true, performElevatedUpdate(ctx, root, digest, operation, current, self)
	}
	if len(args) != 1 {
		return true, fmt.Errorf("普通更新辅助命令不接受其他参数")
	}
	if discover == nil {
		return true, fmt.Errorf("缺少原用户数据目录")
	}
	dataDirectory, err := discover()
	if err != nil {
		return true, err
	}
	root, err := openRoot(filepath.Join(dataDirectory, "updates"))
	if err != nil {
		return true, err
	}
	defer root.Close()
	if args[0] == "--apply-update" {
		return true, runApplyHelper(ctx, root, current, self, restartUpdateApplication)
	}
	return true, runRecoveryRelaunch(ctx, root, current, self, restartUpdateApplication)
}

func restartUpdateApplication(executable string) error {
	if !cleanInstallPath(executable) {
		return fmt.Errorf("重启应用路径无效")
	}
	command := exec.Command(executable, "--gui")
	command.Dir = filepath.Dir(executable)
	if err := command.Start(); err != nil {
		return fmt.Errorf("无法重新启动应用：%w", err)
	}
	// 子进程是新 GUI；普通协调器不等待其整个生命周期。
	go func() { _ = command.Wait() }()
	return nil
}
