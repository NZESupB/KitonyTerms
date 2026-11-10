package update

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// 保持 Rust 的 [parent_pid, executable, source_sha256, restore] 记录格式。
type recoveryRelaunch struct {
	ParentPID    uint32
	Executable   string
	SourceSHA256 string
	Restore      bool
}

func (record recoveryRelaunch) MarshalJSON() ([]byte, error) {
	return json.Marshal([]any{record.ParentPID, record.Executable, record.SourceSHA256, record.Restore})
}

func readRecoveryRelaunch(root *os.Root) (record recoveryRelaunch, digest string, err error) {
	data, err := readPrivate(root, "relaunch.json", 64<<10)
	if err != nil {
		return record, "", err
	}
	var fields []json.RawMessage
	if err = decodeJSON(data, &fields, true); err != nil || len(fields) != 4 {
		return record, "", fmt.Errorf("恢复重启记录字段无效")
	}
	for _, field := range fields {
		if string(field) == "null" {
			return record, "", fmt.Errorf("恢复重启记录不能包含 null")
		}
	}
	if json.Unmarshal(fields[0], &record.ParentPID) != nil || json.Unmarshal(fields[1], &record.Executable) != nil || json.Unmarshal(fields[2], &record.SourceSHA256) != nil || json.Unmarshal(fields[3], &record.Restore) != nil || record.ParentPID == 0 || record.ParentPID > 0x7fffffff || !cleanInstallPath(record.Executable) || !validHex(record.SourceSHA256, 64) {
		return record, "", fmt.Errorf("恢复重启记录身份无效")
	}
	sum := sha256.Sum256(data)
	return record, hex.EncodeToString(sum[:]), nil
}

// 目标可能暂时缺失；此时只规范化现存父目录，不借链接重定向目标名称。
func sameRecoveryExecutable(first, second string) bool {
	if !cleanInstallPath(first) || !cleanInstallPath(second) {
		return false
	}
	if sameInstallPath(first, second) {
		return true
	}
	return filepath.Base(first) == filepath.Base(second) && sameInstallPath(filepath.Dir(first), filepath.Dir(second))
}

func recoveredInstallation(executable string) installation {
	return installationAt(executable, runtime.GOOS)
}

type relaunchIdentity struct {
	Record        recoveryRelaunch
	RecordSHA256  string
	JournalSHA256 string
	Task          installTask
	HasTask       bool
}

func validateRecoveryRelaunch(ctx context.Context, root *os.Root, current BuildInfo, self string) (relaunchIdentity, error) {
	record, digest, err := readRecoveryRelaunch(root)
	if err != nil {
		return relaunchIdentity{}, err
	}
	if record.ParentPID == uint32(os.Getpid()) {
		return relaunchIdentity{}, fmt.Errorf("恢复辅助程序不能等待自身退出")
	}
	identity, err := inspectRecoveryRelaunch(ctx, root, current, self, record)
	identity.RecordSHA256 = digest
	return identity, err
}

func inspectRecoveryRelaunch(ctx context.Context, root *os.Root, current BuildInfo, self string, record recoveryRelaunch) (identity relaunchIdentity, err error) {
	identity.Record = record
	if err = current.Validate(); err != nil || current.Target != desktopTarget() {
		return identity, errors.Join(err, fmt.Errorf("恢复辅助程序构建身份无效"))
	}
	canonical, err := canonicalUpdatePath(self)
	if err != nil {
		return identity, err
	}
	rootCanonical, err := canonicalUpdatePath(root.Name())
	if err != nil {
		return identity, err
	}
	if canonical != filepath.Join(rootCanonical, updateHelperName()) {
		return identity, fmt.Errorf("恢复辅助程序位置无效")
	}
	digest, err := hashInstallFile(ctx, root, updateHelperName())
	if err != nil || digest != strings.ToLower(record.SourceSHA256) {
		return identity, errors.Join(err, fmt.Errorf("恢复辅助程序摘要不匹配"))
	}
	if present, checkErr := markerPresent(root, "task.json"); checkErr != nil {
		return identity, checkErr
	} else if present {
		identity.Task, err = readInstallTask(root)
		if err != nil {
			return identity, err
		}
		identity.HasTask = true
		if identity.Task.Current != current || !strings.EqualFold(identity.Task.SourceSHA256, digest) || !sameRecoveryExecutable(identity.Task.Installation.Executable, record.Executable) {
			return identity, fmt.Errorf("恢复重启与原安装任务不匹配")
		}
	}
	install := recoveredInstallation(record.Executable)
	present, err := markerPresent(root, "journal.json")
	if err != nil {
		return identity, err
	}
	if record.Restore {
		journal, journalDigest, readErr := readInstallJournal(root)
		if readErr != nil {
			return identity, readErr
		}
		persisted := identity.Task
		persisted.Elevate = journal.Task.Elevate
		if journal.Task.Current != current || !strings.EqualFold(journal.Task.SourceSHA256, digest) || !sameRecoveryExecutable(journal.Task.Installation.Executable, record.Executable) || identity.HasTask && persisted != journal.Task {
			return identity, fmt.Errorf("恢复重启与安装记录身份不匹配")
		}
		identity.JournalSHA256 = journalDigest
		install = journal.Task.Installation
	} else if present {
		return identity, fmt.Errorf("仍有安装记录，不能跳过恢复重开应用")
	}
	parent, err := openInstallParent(install, root)
	if err != nil {
		return identity, err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	if !record.Restore {
		err = verifyInstallPayload(ctx, parent, filepath.Base(install.Target), install.Bundle, current, record.SourceSHA256)
	}
	return identity, err
}

func (i *installer) prepareRecoveryRelaunch(ctx context.Context, install installation, source BuildInfo, restore bool) (record recoveryRelaunch, err error) {
	if err = i.held(); err != nil {
		return record, err
	}
	lease, err := acquireInstallLock(ctx, i.root, "helper.lock", 0)
	if err != nil {
		return record, err
	}
	defer func() { err = errors.Join(err, lease.Close()) }()
	if err = requireCleanHelperMarkers(i.root); err != nil {
		return record, err
	}
	digest, err := hashInstallFile(ctx, i.root, updateHelperName())
	if err != nil {
		return record, err
	}
	// 父进程可能仍运行新映像；恢复 helper 必须是 journal 指定的旧构建。
	if err = verifyBuildCommand(ctx, filepath.Join(i.root.Name(), updateHelperName()), source); err != nil {
		return record, err
	}
	record = recoveryRelaunch{ParentPID: uint32(os.Getpid()), Executable: install.Executable, SourceSHA256: digest, Restore: restore}
	if _, err = inspectRecoveryRelaunch(ctx, i.root, source, filepath.Join(i.root.Name(), updateHelperName()), record); err != nil {
		return record, err
	}
	return record, writeInstallRecord(i.root, "relaunch.json", record)
}

func cleanupRelaunchMarkers(root *os.Root) error {
	for _, name := range []string{"relaunch.json", "helper-ready.json", "commit.json", "abort.json"} {
		if err := removeInstallEntry(root, name); err != nil {
			return err
		}
	}
	return syncInstallDirectory(root, ".")
}

func spawnRecoveryRelaunch(ctx context.Context, root *os.Root, record recoveryRelaunch) (*updateHelperProcess, error) {
	persisted, _, err := readRecoveryRelaunch(root)
	if err != nil || persisted != record || record.ParentPID != uint32(os.Getpid()) {
		return nil, errors.Join(err, errTaskMismatch)
	}
	return spawnUpdateHelper(ctx, root, "--relaunch-update", record.SourceSHA256)
}

// 普通权限协调器持锁恢复；失败保留 journal/备份/暂存及原始恢复记录。
func runRecoveryRelaunch(ctx context.Context, root *os.Root, current BuildInfo, self string, restart func(string) error) (err error) {
	helper, err := acquireInstallLock(ctx, root, "helper.lock", 0)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, helper.Close()) }()
	identity, err := validateRecoveryRelaunch(ctx, root, current, self)
	if err != nil {
		return err
	}
	record := identity.Record
	if err = writeInstallRecord(root, "helper-ready.json", []any{uint32(os.Getpid()), record.SourceSHA256}); err != nil {
		return err
	}
	approved, err := waitForUpdateRestart(ctx, root, record.ParentPID, 60*time.Second)
	if err != nil {
		return err
	}
	if !approved {
		return cleanupRelaunchMarkers(root)
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
	fresh, err := validateRecoveryRelaunch(ctx, root, current, self)
	if err != nil || fresh != identity {
		return errors.Join(err, fmt.Errorf("恢复重启记录在等待期间变化"))
	}
	engine := &installer{root: root, lock: gate}
	if record.Restore {
		if err = engine.rollback(ctx); err != nil {
			return errors.Join(err, writeInstallRecord(root, "error.json", "更新恢复尚未完成："+err.Error()))
		}
		if err = writeInstallRecord(root, "error.json", "上次更新被中断，已恢复旧版本"); err != nil {
			return err
		}
	}
	install := recoveredInstallation(record.Executable)
	if err = verifyRestoredInstall(ctx, root, installTask{Installation: install, SourceSHA256: record.SourceSHA256}); err != nil {
		return err
	}
	if err = verifyBuildCommand(ctx, record.Executable, current); err != nil {
		return err
	}
	if restart == nil {
		return fmt.Errorf("缺少原生应用重启入口")
	}
	if err = cleanupUpdateMarkers(root); err != nil {
		return err
	}
	if err = errors.Join(instance.Close(), gate.Close(), helper.Close()); err != nil {
		return err
	}
	return restart(record.Executable)
}
