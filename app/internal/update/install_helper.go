package update

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

func updateHelperName() string {
	if runtime.GOOS == "windows" {
		return "update-helper.exe"
	}
	return "update-helper"
}

func markerPresent(root *os.Root, name string) (bool, error) {
	_, err := root.Lstat(name)
	if errors.Is(err, fs.ErrNotExist) {
		return false, nil
	}
	return err == nil, err
}

func restartCommitted(root *os.Root) error {
	data, err := readPrivate(root, "commit.json", 1024)
	if err != nil {
		return fmt.Errorf("旧进程未确认重启：%w", err)
	}
	var value bool
	if err = decodeJSON(data, &value, true); err != nil || !value {
		return fmt.Errorf("更新重启确认标记无效")
	}
	return nil
}

// ready 只确认 helper 已校验任务；必须同时等旧进程退出和明确的 commit。
func waitForUpdateRestart(ctx context.Context, root *os.Root, pid uint32, budget time.Duration) (bool, error) {
	deadline, cancel := context.WithTimeout(ctx, budget)
	defer cancel()
	for {
		if err := deadline.Err(); err != nil {
			return false, err
		}
		abort, err := markerPresent(root, "abort.json")
		if err != nil {
			return false, err
		}
		if abort {
			return false, nil
		}
		alive, err := updateProcessAlive(pid)
		if err != nil {
			return false, err
		}
		if !alive {
			if err = restartCommitted(root); err != nil {
				return false, err
			}
			// commit 读取期间到达的 abort 仍然优先，不能把取消解释为重启。
			abort, err = markerPresent(root, "abort.json")
			return !abort, err
		}
		timer := time.NewTimer(50 * time.Millisecond)
		select {
		case <-deadline.Done():
			timer.Stop()
			return false, deadline.Err()
		case <-timer.C:
		}
	}
}

func readHelperReady(root *os.Root, pid uint32, digest string) error {
	data, err := readPrivate(root, "helper-ready.json", 1024)
	if err != nil {
		return err
	}
	var fields []json.RawMessage
	if err = decodeJSON(data, &fields, true); err != nil || len(fields) != 2 {
		return fmt.Errorf("辅助程序握手字段无效")
	}
	var actualPID uint32
	var actualDigest string
	if json.Unmarshal(fields[0], &actualPID) != nil || json.Unmarshal(fields[1], &actualDigest) != nil || actualPID != pid || actualPID == 0 || actualDigest != digest || !validHex(actualDigest, 64) {
		return fmt.Errorf("辅助程序PID或摘要握手不匹配")
	}
	return nil
}

func validateApplyHelper(ctx context.Context, root *os.Root, current BuildInfo, helper string) (installTask, error) {
	task, err := readInstallTask(root)
	if err != nil {
		return task, err
	}
	canonical, err := canonicalUpdatePath(helper)
	if err != nil {
		return task, err
	}
	rootCanonical, err := canonicalUpdatePath(root.Name())
	if err != nil {
		return task, err
	}
	if canonical != filepath.Join(rootCanonical, updateHelperName()) || task.Current != current || task.ParentPID == uint32(os.Getpid()) {
		return task, fmt.Errorf("更新辅助程序位置或构建身份不匹配")
	}
	if present, err := markerPresent(root, "journal.json"); err != nil {
		return task, err
	} else if present {
		return task, fmt.Errorf("存在未恢复的安装记录，拒绝新helper任务")
	}
	digest, err := hashInstallFile(ctx, root, updateHelperName())
	if err != nil || digest != strings.ToLower(task.SourceSHA256) {
		return task, errors.Join(err, fmt.Errorf("更新辅助程序摘要不匹配"))
	}
	if task.Installation.Installed {
		if err = registeredInstallation(task.Installation.Executable); err != nil {
			return task, err
		}
	}
	parent, err := openInstallParent(task.Installation, root)
	if err != nil {
		return task, err
	}
	defer parent.Close()
	digest, err = hashInstallFile(ctx, parent, installExecutable(filepath.Base(task.Installation.Target), task.Installation.Bundle))
	if err != nil || digest != strings.ToLower(task.SourceSHA256) {
		return task, errors.Join(err, fmt.Errorf("旧版本安装目标已改变"))
	}
	file, err := openVerifiedPackage(ctx, root, task.Package)
	if err != nil {
		return task, err
	}
	return task, file.Close()
}

func (i *installer) installHelper(ctx context.Context, task installTask) (err error) {
	if err = i.held(); err != nil {
		return err
	}
	persisted, err := readInstallTask(i.root)
	if err != nil || persisted != task || task.ParentPID != uint32(os.Getpid()) {
		return errors.Join(err, errTaskMismatch)
	}
	lease, err := acquireInstallLock(ctx, i.root, "helper.lock", 0)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, lease.Close()) }()
	parent, err := openInstallParent(task.Installation, i.root)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	file, name, err := privateTemp(i.root)
	if err != nil {
		return err
	}
	if err = file.Close(); err != nil {
		return err
	}
	if err = i.root.Remove(name); err != nil {
		return err
	}
	defer func() { err = errors.Join(err, removeInstallEntry(i.root, name)) }()
	if err = copyInstallFile(ctx, parent, installExecutable(filepath.Base(task.Installation.Target), task.Installation.Bundle), i.root, name); err != nil {
		return err
	}
	digest, err := hashInstallFile(ctx, i.root, name)
	if err != nil || digest != strings.ToLower(task.SourceSHA256) {
		return errors.Join(err, fmt.Errorf("辅助程序副本校验失败"))
	}
	if err = platformReplaceInstall(ctx, i.root, name, updateHelperName(), false); err != nil {
		return err
	}
	return syncInstallDirectory(i.root, ".")
}

type updateHelperProcess struct {
	command *exec.Cmd
	done    chan struct{}
	err     error
}

func spawnApplyHelper(ctx context.Context, root *os.Root, digest string) (*updateHelperProcess, error) {
	task, err := readInstallTask(root)
	if err != nil || task.ParentPID != uint32(os.Getpid()) || task.SourceSHA256 != digest {
		return nil, errors.Join(err, errTaskMismatch)
	}
	if present, err := markerPresent(root, "relaunch.json"); err != nil || present {
		return nil, errors.Join(err, fmt.Errorf("存在恢复重启记录，拒绝新安装helper"))
	}
	return spawnUpdateHelper(ctx, root, "--apply-update", digest)
}

func requireCleanHelperMarkers(root *os.Root) error {
	for _, name := range []string{"commit.json", "abort.json", "helper-ready.json", "relaunch.json"} {
		if present, err := markerPresent(root, name); err != nil || present {
			return errors.Join(err, fmt.Errorf("更新辅助程序启动前存在旧标记"))
		}
	}
	return nil
}

func spawnUpdateHelper(ctx context.Context, root *os.Root, argument, digest string) (*updateHelperProcess, error) {
	if argument != "--apply-update" && argument != "--relaunch-update" || !validHex(digest, 64) {
		return nil, fmt.Errorf("更新辅助程序启动参数无效")
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	for _, name := range []string{"commit.json", "abort.json", "helper-ready.json"} {
		if present, err := markerPresent(root, name); err != nil || present {
			return nil, errors.Join(err, fmt.Errorf("更新辅助程序启动前存在旧标记"))
		}
	}
	actual, err := hashInstallFile(ctx, root, updateHelperName())
	if err != nil || actual != strings.ToLower(digest) {
		return nil, errors.Join(err, fmt.Errorf("待启动辅助程序摘要不匹配"))
	}
	command := exec.Command(filepath.Join(root.Name(), updateHelperName()), argument)
	command.Dir = root.Name()
	if err = command.Start(); err != nil {
		return nil, err
	}
	process := &updateHelperProcess{command: command, done: make(chan struct{})}
	go func() { process.err = command.Wait(); close(process.done) }()
	deadline, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	for {
		present, checkErr := markerPresent(root, "helper-ready.json")
		if checkErr == nil && present {
			checkErr = readHelperReady(root, uint32(command.Process.Pid), digest)
			if checkErr == nil {
				return process, nil
			}
		}
		if checkErr != nil {
			err = checkErr
			break
		}
		timer := time.NewTimer(50 * time.Millisecond)
		select {
		case <-deadline.Done():
			timer.Stop()
			err = deadline.Err()
		case <-process.done:
			timer.Stop()
			return nil, errors.Join(fmt.Errorf("辅助程序在握手前退出"), process.err)
		case <-timer.C:
			continue
		}
		break
	}
	// 此时旧进程仍活着，且尚未写入 commit，停止未授权的 helper 后必须回收进程。
	killErr := command.Process.Kill()
	if errors.Is(killErr, os.ErrProcessDone) {
		killErr = nil
	}
	<-process.done
	return nil, errors.Join(err, killErr)
}

func cleanupUpdateMarkers(root *os.Root) error {
	for _, name := range []string{"task.json", "helper-ready.json", "commit.json", "abort.json", "relaunch.json"} {
		if err := removeInstallEntry(root, name); err != nil {
			return err
		}
	}
	return syncInstallDirectory(root, ".")
}

// restart 仅由原生启动层提供；正式辅助命令复用此协调器。
func runApplyHelper(ctx context.Context, root *os.Root, current BuildInfo, self string, restart func(string) error) (err error) {
	helper, err := acquireInstallLock(ctx, root, "helper.lock", 0)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, helper.Close()) }()
	task, err := validateApplyHelper(ctx, root, current, self)
	if err != nil {
		return err
	}
	if err = writeInstallRecord(root, "helper-ready.json", []any{uint32(os.Getpid()), task.SourceSHA256}); err != nil {
		return err
	}
	approved, err := waitForUpdateRestart(ctx, root, task.ParentPID, 60*time.Second)
	if err != nil {
		return err
	}
	if !approved {
		return cleanupPreparationRecords(root, task, uint32(os.Getpid()))
	}
	gate, err := acquireInstallLock(ctx, root, "installation.lock", 10*time.Second)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, gate.Close()) }()
	data, err := os.OpenRoot(filepath.Dir(root.Name()))
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, data.Close()) }()
	instance, err := acquireInstallLock(ctx, data, "kitonyterms.lock", 10*time.Second)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, instance.Close()) }()
	// 等待期间可能有人改写任务；重新确认后才进入安装事务。
	validated, err := validateApplyHelper(ctx, root, current, self)
	if err != nil || validated != task {
		return errors.Join(err, errTaskMismatch)
	}
	engine := &installer{root: root, lock: gate}
	if applyErr := engine.apply(ctx, task); applyErr != nil {
		recoveryCtx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
		recoveryErr := engine.rollback(recoveryCtx)
		if recoveryErr == nil {
			recoveryErr = verifyRestoredInstall(recoveryCtx, root, task)
		}
		cancel()
		message := "更新失败，已保留或恢复旧版本"
		if recoveryErr != nil {
			message = "更新失败，恢复尚未完成"
		}
		recordErr := writeInstallRecord(root, "error.json", message+"："+applyErr.Error())
		if recoveryErr != nil || recordErr != nil {
			return errors.Join(applyErr, recoveryErr, recordErr)
		}
	}
	if err = cleanupUpdateMarkers(root); err != nil {
		return err
	}
	if err = errors.Join(instance.Close(), gate.Close(), helper.Close()); err != nil {
		return err
	}
	if restart == nil {
		return fmt.Errorf("缺少原生应用重启入口")
	}
	return restart(task.Installation.Executable)
}

func verifyRestoredInstall(ctx context.Context, root *os.Root, task installTask) (err error) {
	parent, err := openInstallParent(task.Installation, root)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	digest, err := hashInstallFile(ctx, parent, installExecutable(filepath.Base(task.Installation.Target), task.Installation.Bundle))
	if err != nil || digest != strings.ToLower(task.SourceSHA256) {
		return errors.Join(err, fmt.Errorf("原程序未恢复到已验证身份"))
	}
	if task.Installation.Bundle {
		return verifyBundle(ctx, task.Installation.Target)
	}
	return nil
}
