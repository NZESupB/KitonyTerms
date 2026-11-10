package update

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// 每个实例持有 installation.lock；helper 还须独立持有 helper.lock。
// 此引擎不对外暴露任意安装路径，接线前只用于隔离安装副本验证。
type installer struct {
	root       *os.Root
	lock       *installLock
	checkpoint func(string) error
}

func openInstaller(ctx context.Context, root *os.Root) (*installer, error) {
	lock, err := acquireInstallLock(ctx, root, "installation.lock", 0)
	if err != nil {
		return nil, err
	}
	return &installer{root: root, lock: lock}, nil
}
func (i *installer) Close() error { return i.lock.Close() }
func (i *installer) held() error {
	if i == nil || i.lock == nil || i.lock.file == nil {
		return fmt.Errorf("安装操作缺少互斥锁")
	}
	return nil
}
func (i *installer) at(stage string) error {
	if i.checkpoint != nil {
		return i.checkpoint(stage)
	}
	return nil
}
func (i *installer) writeJournal(journal installJournal) error {
	if err := journal.validate(i.root); err != nil {
		return err
	}
	return writeInstallRecord(i.root, "journal.json", journal)
}
func (i *installer) backup(journal installJournal, parent *os.Root) (*os.Root, string) {
	if journal.Storage == "user_directory" || journal.Task.Installation.Installed {
		return i.root, "previous-executable.exe"
	}
	return parent, "." + filepath.Base(journal.Task.Installation.Target) + "-update-backup"
}
func (i *installer) stagingRoot(journal installJournal, parent *os.Root) (*os.Root, string) {
	if journal.Storage == "user_directory" || journal.Task.Installation.Installed {
		return i.root, filepath.Base(journal.Staging)
	}
	return parent, filepath.Base(journal.Staging)
}

func (i *installer) apply(ctx context.Context, task installTask) (err error) {
	if err = i.held(); err != nil {
		return err
	}
	if err = task.validate(); err != nil {
		return err
	}
	var prepared preparedPayload
	if task.Installation.Installed {
		prepared, err = i.prepareInstallerPayload(ctx, task)
	} else {
		prepared, err = extractVerifiedPayload(ctx, i.root, task.Package, task.Next)
	}
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, removeInstallEntry(i.root, prepared.Directory)) }()
	return i.applyPrepared(ctx, task, prepared)
}

func (i *installer) applyPrepared(ctx context.Context, task installTask, prepared preparedPayload) (err error) {
	if err = i.held(); err != nil {
		return err
	}
	if err = task.validate(); err != nil {
		return err
	}
	if _, err = i.root.Lstat("journal.json"); err == nil {
		return fmt.Errorf("已有安装记录，必须先恢复")
	} else if !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	if persisted, readErr := readInstallTask(i.root); readErr != nil || persisted != task {
		return fmt.Errorf("安装任务与持久化记录不一致：%w", errors.Join(readErr, errTaskMismatch))
	}
	if !strings.HasPrefix(prepared.Directory, ".payload-") || filepath.Base(prepared.Directory) != prepared.Directory || filepath.ToSlash(filepath.Clean(prepared.Payload)) != prepared.Payload || !strings.HasPrefix(prepared.Payload, prepared.Directory+"/") || !validHex(prepared.SHA256, 64) {
		return fmt.Errorf("安装载荷不属于私有准备目录")
	}
	install := task.Installation
	parent, err := openInstallParent(install, i.root)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	target := filepath.Base(install.Target)
	original, err := hashInstallFile(ctx, parent, installExecutable(target, install.Bundle))
	if err != nil {
		return err
	}
	if original != strings.ToLower(task.SourceSHA256) {
		return fmt.Errorf("安装目标已变化")
	}
	journal := installJournal{Task: task, Stage: "prepared", Storage: "legacy"}
	if runtime.GOOS == "windows" {
		journal.Storage = "user_directory"
	}
	stageParent := parent
	if journal.Storage == "user_directory" {
		stageParent = i.root
	}
	backupRoot, backup := i.backup(journal, parent)
	if _, err = backupRoot.Lstat(backup); err == nil {
		return fmt.Errorf("备份路径已经存在")
	} else if !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	stage := ".kt-update-stage-" + rand.Text()
	if err = stageParent.Mkdir(stage, 0700); err != nil {
		return err
	}
	journal.Staging = filepath.Join(stageParent.Name(), stage)
	// journal 落盘后由恢复流程拥有暂存目录，即使随后返回错误也不提前删除。
	recorded := false
	defer func() {
		if !recorded {
			err = errors.Join(err, removeInstallEntry(stageParent, stage))
		}
	}()
	stageRoot, err := stageParent.OpenRoot(stage)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, stageRoot.Close()) }()
	if err = copyInstallPayload(ctx, i.root, prepared.Payload, stageRoot, "payload", install.Bundle); err != nil {
		return err
	}
	if !install.Bundle {
		mode, modeErr := parent.Stat(target)
		if modeErr != nil {
			return modeErr
		}
		if err = chmodInstallFile(stageRoot, "payload", mode.Mode().Perm()); err != nil {
			return err
		}
	}
	stagedBuild := task.Next
	if install.Installed {
		stagedBuild = task.Current
	}
	if err = verifyInstallPayload(ctx, stageRoot, "payload", install.Bundle, stagedBuild, prepared.SHA256); err != nil {
		return err
	}
	if !install.Bundle {
		journal.StagedSHA256 = prepared.SHA256
	}
	if err = syncInstallDirectory(stageRoot, "."); err != nil {
		return err
	}
	if err = syncInstallDirectory(stageParent, "."); err != nil {
		return err
	}
	// Rename 已成功但目录刷盘失败时，同样必须保留 journal 指向的材料。
	err = i.writeJournal(journal)
	_, recordErr := i.root.Lstat("journal.json")
	recorded = recordErr == nil
	if err != nil {
		return err
	}
	if err = i.at("prepared"); err != nil {
		return err
	}
	if err = copyInstallPayload(ctx, parent, target, backupRoot, backup, install.Bundle); err != nil {
		return err
	}
	backupDigest, err := hashInstallFile(ctx, backupRoot, installExecutable(backup, install.Bundle))
	if err != nil || backupDigest != original {
		return errors.Join(err, fmt.Errorf("旧版本备份校验失败"))
	}
	if install.Bundle {
		if err = verifyBundle(ctx, filepath.Join(backupRoot.Name(), backup)); err != nil {
			return err
		}
	}
	if err = syncInstallDirectory(backupRoot, "."); err != nil {
		return err
	}
	if err = i.at("backup_copied"); err != nil {
		return err
	}
	journal.Stage = "backed_up"
	if err = i.writeJournal(journal); err != nil {
		return err
	}
	if err = i.at("backed_up"); err != nil {
		return err
	}
	// 备份之后再次检查旧目标，避免覆盖准备期间被其他安装器改变的程序。
	current, err := hashInstallFile(ctx, parent, installExecutable(target, install.Bundle))
	if err != nil || current != original {
		return errors.Join(err, fmt.Errorf("安装目标在备份后变化"))
	}
	if install.Installed {
		err = i.runWindowsInstaller(ctx, task)
	} else {
		err = i.replaceWithElevation(ctx, &journal, stageRoot, "payload", parent, target, prepared.SHA256, false)
	}
	if err != nil {
		return err
	}
	if err = i.at("replaced"); err != nil {
		return err
	}
	expectedDigest := prepared.SHA256
	if install.Installed {
		expectedDigest = ""
	}
	if err = verifyInstallPayload(ctx, parent, target, install.Bundle, task.Next, expectedDigest); err != nil {
		return err
	}
	if err = i.at("verified"); err != nil {
		return err
	}
	if install.Installed {
		journal.StagedSHA256, err = hashInstallFile(ctx, parent, target)
		if err != nil {
			return err
		}
	}
	journal.Stage = "committed"
	if err = i.writeJournal(journal); err != nil {
		return err
	}
	return i.at("committed")
}

var errTaskMismatch = errors.New("安装任务身份不匹配")

func verifyInstallPayload(ctx context.Context, root *os.Root, name string, bundle bool, expected BuildInfo, digest string) error {
	executable := installExecutable(name, bundle)
	if err := validateBinary(root, executable, expected.Target); err != nil {
		return err
	}
	actual, err := hashInstallFile(ctx, root, executable)
	if err != nil {
		return err
	}
	if digest != "" && actual != strings.ToLower(digest) {
		return fmt.Errorf("安装载荷摘要不匹配")
	}
	if bundle {
		if err := verifyBundle(ctx, filepath.Join(root.Name(), name)); err != nil {
			return err
		}
	}
	return verifyBuildCommand(ctx, filepath.Join(root.Name(), executable), expected)
}

// 无论暂存在哪个卷，都在目标卷复制校验后替换；载荷和备份留待确认。
func (i *installer) replace(ctx context.Context, source *os.Root, name string, parent *os.Root, target string, bundle bool, digest string) (err error) {
	if info, statErr := parent.Lstat(target); statErr == nil && (info.Mode()&fs.ModeSymlink != 0 || bundle && !info.IsDir() || !bundle && !info.Mode().IsRegular()) {
		return fmt.Errorf("替换目标类型已变化")
	} else if statErr != nil && !errors.Is(statErr, fs.ErrNotExist) {
		return statErr
	}
	directory := ".kt-update-replace-" + rand.Text()
	if err = parent.Mkdir(directory, 0700); err != nil {
		return installDirectoryFailure(err, parent, target)
	}
	defer func() { err = errors.Join(err, removeInstallEntry(parent, directory)) }()
	local, err := parent.OpenRoot(directory)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, local.Close()) }()
	if err = copyInstallPayload(ctx, source, name, local, "payload", bundle); err != nil {
		return err
	}
	actual, err := hashInstallFile(ctx, local, installExecutable("payload", bundle))
	if err != nil || actual != strings.ToLower(digest) {
		return errors.Join(err, fmt.Errorf("目标卷副本摘要不匹配"))
	}
	if bundle {
		if err = verifyBundle(ctx, filepath.Join(local.Name(), "payload")); err != nil {
			return err
		}
	}
	if err = syncInstallDirectory(local, "."); err != nil {
		return err
	}
	if err = platformReplaceInstall(ctx, parent, filepath.Join(directory, "payload"), target, bundle); err != nil {
		return err
	}
	return syncInstallDirectory(parent, ".")
}

// 失败协调进程持锁调用。恢复本身失败时保留 journal、备份及暂存目录。
func (i *installer) rollback(ctx context.Context) (err error) {
	if err = i.held(); err != nil {
		return err
	}
	if _, err = i.root.Lstat("journal.json"); errors.Is(err, fs.ErrNotExist) {
		return nil
	} else if err != nil {
		return err
	}
	journal, _, err := readInstallJournal(i.root)
	if err != nil {
		return err
	}
	install := journal.Task.Installation
	parent, err := openInstallParent(install, i.root)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	target := filepath.Base(install.Target)
	backupRoot, backup := i.backup(journal, parent)
	original := strings.ToLower(journal.Task.SourceSHA256)
	current, currentErr := hashInstallFile(ctx, parent, installExecutable(target, install.Bundle))
	if journal.Stage == "prepared" {
		if currentErr != nil || current != original {
			return errors.Join(currentErr, fmt.Errorf("准备阶段目标已变化，保留事务等待人工恢复"))
		}
	} else {
		digest, backupErr := hashInstallFile(ctx, backupRoot, installExecutable(backup, install.Bundle))
		untouchedWithoutBackup := !install.Bundle && errors.Is(backupErr, fs.ErrNotExist) && currentErr == nil && current == original
		if !untouchedWithoutBackup && (backupErr != nil || digest != original) {
			return errors.Join(backupErr, fmt.Errorf("旧版本备份校验失败"))
		}
		if install.Bundle || currentErr != nil || current != original {
			if err = i.replaceWithElevation(ctx, &journal, backupRoot, backup, parent, target, original, true); err != nil {
				return err
			}
		}
		actual, err := hashInstallFile(ctx, parent, installExecutable(target, install.Bundle))
		if err != nil || actual != original {
			return errors.Join(err, fmt.Errorf("恢复结果摘要不匹配"))
		}
		if install.Bundle {
			if err = verifyBundle(ctx, filepath.Join(parent.Name(), target)); err != nil {
				return err
			}
		}
		// 恢复已完成，先记录目标未改变的 prepared 状态，确保清理中断可重入。
		journal.Stage = "prepared"
		if err = i.writeJournal(journal); err != nil {
			return err
		}
	}
	return i.cleanup(journal, parent)
}

func (i *installer) cleanup(journal installJournal, parent *os.Root) error {
	backupRoot, backup := i.backup(journal, parent)
	if err := removeInstallEntry(backupRoot, backup); err != nil {
		return err
	}
	if err := syncInstallDirectory(backupRoot, "."); err != nil {
		return err
	}
	stageRoot, stage := i.stagingRoot(journal, parent)
	if err := removeInstallEntry(stageRoot, stage); err != nil {
		return err
	}
	if err := syncInstallDirectory(stageRoot, "."); err != nil {
		return err
	}
	if err := i.root.Remove("journal.json"); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	return syncInstallDirectory(i.root, ".")
}

// 启动探测不清备份；只有精确匹配的新构建在 GUI 就绪后可 confirm。
func (i *installer) recover(ctx context.Context, current BuildInfo, executable string, confirm bool) (err error) {
	if err = i.held(); err != nil {
		return err
	}
	if _, err = i.root.Lstat("journal.json"); errors.Is(err, fs.ErrNotExist) {
		return nil
	} else if err != nil {
		return err
	}
	journal, _, err := readInstallJournal(i.root)
	if err != nil {
		return err
	}
	if !sameInstallPath(executable, journal.Task.Installation.Executable) {
		return fmt.Errorf("安装记录属于其他应用路径")
	}
	if journal.Stage != "committed" || journal.Task.Next != current {
		if err = i.rollback(ctx); err != nil {
			return err
		}
		return writeInstallRecord(i.root, "error.json", "上次更新被中断，已恢复旧版本")
	}
	install := journal.Task.Installation
	parent, err := openInstallParent(install, i.root)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	if err = verifyInstallPayload(ctx, parent, filepath.Base(install.Target), install.Bundle, current, journal.verificationDigest()); err != nil {
		return err
	}
	if !confirm {
		return nil
	}
	if err = i.cleanup(journal, parent); err != nil {
		return err
	}
	for _, name := range []string{"elevation.json", "error.json", "elevated-result.json"} {
		if err = removeInstallEntry(i.root, name); err != nil {
			return err
		}
	}
	return syncInstallDirectory(i.root, ".")
}

func (i *installer) prepareTask(ctx context.Context, install installation, current BuildInfo, candidate available, elevate bool) (task installTask, err error) {
	if err = i.held(); err != nil {
		return task, err
	}
	if err = pendingInstallation(i.root); err != nil {
		return task, err
	}
	if install.Installed {
		if err = registeredInstallation(install.Executable); err != nil {
			return task, err
		}
	}
	task = installTask{Installation: install, Current: current, Next: candidate.Info, Package: candidate.Package, ParentPID: uint32(os.Getpid()), Elevate: elevate}
	parent, err := openInstallParent(install, i.root)
	if err != nil {
		return task, err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	task.SourceSHA256, err = hashInstallFile(ctx, parent, installExecutable(filepath.Base(install.Target), install.Bundle))
	if err != nil {
		return task, err
	}
	if err = task.validate(); err != nil {
		return task, err
	}
	if err = verifyBuildCommand(ctx, install.Executable, current); err != nil {
		return task, err
	}
	var prepared preparedPayload
	if install.Installed {
		prepared, err = i.prepareInstallerPayload(ctx, task)
	} else {
		prepared, err = preparePayload(ctx, i.root, candidate.Package, candidate.Info)
	}
	if err != nil {
		return task, err
	}
	if err = removeInstallEntry(i.root, prepared.Directory); err != nil {
		return task, err
	}
	if err = ctx.Err(); err != nil {
		return task, err
	}
	return task, writeInstallRecord(i.root, "task.json", task)
}
