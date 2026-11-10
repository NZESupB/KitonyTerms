package update

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

type elevationOperation string

const (
	elevationReplace elevationOperation = "replace"
	elevationRestore elevationOperation = "restore"
)

func (operation elevationOperation) argument() string {
	if operation == elevationReplace {
		return "--elevated-replace"
	}
	if operation == elevationRestore {
		return "--restore-update"
	}
	return ""
}

type elevatedOutcome struct {
	Operation     elevationOperation `json:"operation"`
	JournalSHA256 string             `json:"journal_sha256"`
	Error         *string            `json:"error"`
}

func parseElevationArguments(args []string) (elevationOperation, string, string, error) {
	if len(args) != 3 || !cleanInstallPath(args[1]) || !validHex(args[2], 64) {
		return "", "", "", fmt.Errorf("管理员入口事务参数无效")
	}
	var operation elevationOperation
	switch args[0] {
	case "--elevated-replace":
		operation = elevationReplace
	case "--restore-update":
		operation = elevationRestore
	default:
		return "", "", "", fmt.Errorf("管理员操作无效")
	}
	return operation, args[1], strings.ToLower(args[2]), nil
}

func checkElevationState(journal installJournal, operation elevationOperation) error {
	if journal.Task.Installation.Bundle {
		return fmt.Errorf("管理员操作不支持应用包")
	}
	switch operation {
	case elevationReplace:
		if !journal.Task.Elevate || journal.Task.Installation.Installed || journal.Stage != "backed_up" || !validHex(journal.StagedSHA256, 64) {
			return fmt.Errorf("提权替换事务阶段无效")
		}
	case elevationRestore:
		if journal.Stage != "backed_up" && journal.Stage != "committed" {
			return fmt.Errorf("提权恢复事务阶段无效")
		}
	default:
		return fmt.Errorf("管理员操作无效")
	}
	return nil
}

func validateElevatedRequest(ctx context.Context, root *os.Root, journal installJournal, current BuildInfo, self string, operation elevationOperation) error {
	if err := journal.validate(root); err != nil {
		return err
	}
	if err := checkElevationState(journal, operation); err != nil {
		return err
	}
	canonical, err := canonicalUpdatePath(self)
	if err != nil {
		return err
	}
	rootPath, err := canonicalUpdatePath(root.Name())
	if err != nil {
		return err
	}
	if canonical != filepath.Join(rootPath, updateHelperName()) || journal.Task.Current != current {
		return fmt.Errorf("管理员辅助程序位置或构建身份不匹配")
	}
	digest, err := hashInstallFile(ctx, root, updateHelperName())
	if err != nil || digest != strings.ToLower(journal.Task.SourceSHA256) {
		return errors.Join(err, fmt.Errorf("管理员辅助程序摘要不匹配"))
	}
	if journal.Task.Installation.Installed {
		if err = registeredInstallation(journal.Task.Installation.Executable); err != nil {
			return err
		}
	}
	if operation == elevationReplace {
		if !installTargetAllowsElevation(journal.Task.Installation.Target) {
			return fmt.Errorf("只读或非普通目标不能提权替换")
		}
		if err = verifyRestoredInstall(ctx, root, journal.Task); err != nil {
			return err
		}
		file, err := openVerifiedPackage(ctx, root, journal.Task.Package)
		if err != nil {
			return err
		}
		return file.Close()
	}
	return nil
}

func readElevatedOutcome(root *os.Root, operation elevationOperation, digest string) error {
	data, err := readPrivate(root, "elevated-result.json", 64*1024)
	if err != nil {
		return fmt.Errorf("管理员程序未返回结果：%w", err)
	}
	var outcome elevatedOutcome
	if err = decodeJSON(data, &outcome, true); err != nil {
		return err
	}
	var fields map[string]json.RawMessage
	if err = json.Unmarshal(data, &fields); err != nil {
		return err
	}
	for _, name := range []string{"operation", "journal_sha256", "error"} {
		if _, ok := fields[name]; !ok {
			return fmt.Errorf("管理员结果缺少字段")
		}
	}
	if outcome.Operation != operation || outcome.JournalSHA256 != digest {
		return fmt.Errorf("管理员结果操作或事务摘要不匹配")
	}
	if outcome.Error != nil {
		return fmt.Errorf("管理员操作失败：%s", *outcome.Error)
	}
	return nil
}

// 返回前必须确认子进程已退出；预算/取消只改变结果，不释放给回滚流程。
func waitElevatedExit(ctx context.Context, budget time.Duration, poll func() (bool, uint32, error)) error {
	deadline := time.Now().Add(budget)
	var pending error
	recordedPollError := false
	for {
		exited, code, err := poll()
		if err != nil && !recordedPollError {
			pending = errors.Join(pending, err)
			recordedPollError = true
		}
		if ctx.Err() != nil && !errors.Is(pending, ctx.Err()) {
			pending = errors.Join(pending, ctx.Err())
		}
		if !time.Now().Before(deadline) && !errors.Is(pending, context.DeadlineExceeded) {
			pending = errors.Join(pending, context.DeadlineExceeded)
		}
		if exited {
			if code != 0 {
				pending = errors.Join(pending, fmt.Errorf("管理员程序失败（退出码%d）", code))
			}
			return pending
		}
		// poll 必须有界；等待错误也不能把仍存活的写入者留给另一个恢复进程。
		time.Sleep(50 * time.Millisecond)
	}
}

func (i *installer) requestElevation(ctx context.Context, operation elevationOperation) error {
	if err := i.held(); err != nil {
		return err
	}
	journal, digest, err := readInstallJournal(i.root)
	if err != nil {
		return err
	}
	if err = checkElevationState(journal, operation); err != nil {
		return err
	}
	if operation == elevationReplace && !installTargetAllowsElevation(journal.Task.Installation.Target) {
		return fmt.Errorf("只读或非普通目标不能提权替换")
	}
	if err = removeInstallEntry(i.root, "elevated-result.json"); err != nil {
		return err
	}
	rootPath, err := canonicalUpdatePath(i.root.Name())
	if err != nil {
		return err
	}
	result := runElevatedUpdate(ctx, filepath.Join(rootPath, updateHelperName()), []string{operation.argument(), rootPath, digest}, journal.Task.SourceSHA256, 5*time.Minute)
	return errors.Join(result, readElevatedOutcome(i.root, operation, digest))
}
