package update

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

type installFixture struct {
	installer    *installer
	root, parent *os.Root
	task         installTask
	prepared     preparedPayload
	original     []byte
}

func newInstallFixture(t *testing.T) installFixture {
	t.Helper()
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	root, err := openRoot(filepath.Join(t.TempDir(), "updates"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := root.Close(); err != nil {
			t.Error(err)
		}
	})
	parent := archiveTestRoot(t)
	engine, err := openInstaller(context.Background(), root)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := engine.Close(); err != nil {
			t.Error(err)
		}
	})
	journal := testInstallJournal(t, root)
	target := filepath.Join(parent.Name(), "kitonyterms")
	journal.Task.Installation = installation{Executable: target, Target: target}
	original := []byte("隔离安装副本的旧内容\n")
	if err = parent.WriteFile("kitonyterms", original, 0700); err != nil {
		t.Fatal(err)
	}
	journal.Task.SourceSHA256, err = hashInstallFile(context.Background(), parent, "kitonyterms")
	if err != nil {
		t.Fatal(err)
	}
	if err = writeInstallRecord(root, "task.json", journal.Task); err != nil {
		t.Fatal(err)
	}
	if err = root.Mkdir(".payload-fixture", 0700); err != nil {
		t.Fatal(err)
	}
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	binary, err := os.ReadFile(executable)
	if err != nil {
		t.Fatal(err)
	}
	if err = root.WriteFile(".payload-fixture/kitonyterms", binary, 0700); err != nil {
		t.Fatal(err)
	}
	digest, err := hashInstallFile(context.Background(), root, ".payload-fixture/kitonyterms")
	if err != nil {
		t.Fatal(err)
	}
	return installFixture{engine, root, parent, journal.Task, preparedPayload{Directory: ".payload-fixture", Payload: ".payload-fixture/kitonyterms", Executable: ".payload-fixture/kitonyterms", SHA256: digest}, original}
}

func assertInstallOriginal(t *testing.T, fixture installFixture) {
	t.Helper()
	data, err := fixture.parent.ReadFile("kitonyterms")
	if err != nil || string(data) != string(fixture.original) {
		t.Fatal("没有保留或恢复原程序", err)
	}
}

func TestInstallTransactionFaultRecovery(t *testing.T) {
	for _, point := range []string{"prepared", "backup_copied", "backed_up", "replaced", "verified", "committed"} {
		t.Run(point, func(t *testing.T) {
			fixture := newInstallFixture(t)
			injected := errors.New("模拟进程在持久化边界中断")
			fixture.installer.checkpoint = func(stage string) error {
				if stage == point {
					return injected
				}
				return nil
			}
			if err := fixture.installer.applyPrepared(context.Background(), fixture.task, fixture.prepared); !errors.Is(err, injected) {
				t.Fatal("未到达中断点", err)
			}
			journal, _, err := readInstallJournal(fixture.root)
			if err != nil {
				t.Fatal("中断后记录不可恢复", err)
			}
			if _, err = os.Stat(journal.Staging); err != nil {
				t.Fatal("中断后提前删除了暂存", err)
			}
			// 重新打开锁和引擎，模拟另一进程从磁盘恢复，而非复用内存状态。
			if err = fixture.installer.Close(); err != nil {
				t.Fatal(err)
			}
			fresh, err := openInstaller(context.Background(), fixture.root)
			if err != nil {
				t.Fatal(err)
			}
			defer fresh.Close()
			if err = fresh.recover(context.Background(), fixture.task.Current, fixture.task.Installation.Executable, false); err != nil {
				t.Fatal("恢复失败", err)
			}
			assertInstallOriginal(t, fixture)
			if _, err = fixture.root.Lstat("journal.json"); !errors.Is(err, fs.ErrNotExist) {
				t.Fatal("恢复后记录未清理", err)
			}
			if _, err = fixture.root.Lstat("error.json"); err != nil {
				t.Fatal("没有保留中断提示", err)
			}
			if err = fresh.rollback(context.Background()); err != nil {
				t.Fatal("恢复不幂等", err)
			}
		})
	}
}

func TestInstallCommittedNeedsExactStartupConfirmation(t *testing.T) {
	fixture := newInstallFixture(t)
	if err := fixture.installer.applyPrepared(context.Background(), fixture.task, fixture.prepared); err != nil {
		t.Fatal(err)
	}
	journal, _, err := readInstallJournal(fixture.root)
	if err != nil || journal.Stage != "committed" {
		t.Fatal("未提交事务", err)
	}
	backupRoot, backup := fixture.installer.backup(journal, fixture.parent)
	for _, confirm := range []bool{false, false} {
		if err = fixture.installer.recover(context.Background(), fixture.task.Next, fixture.task.Installation.Executable, confirm); err != nil {
			t.Fatal(err)
		}
		if _, err = backupRoot.Lstat(backup); err != nil {
			t.Fatal("GUI确认前备份已删除", err)
		}
	}
	if err = fixture.installer.recover(context.Background(), fixture.task.Next, fixture.task.Installation.Executable+"-foreign", true); err == nil {
		t.Fatal("其他安装路径确认了事务")
	}
	if _, err = backupRoot.Lstat(backup); err != nil {
		t.Fatal("错误确认删除了备份", err)
	}
	if err = writeInstallRecord(fixture.root, "error.json", "先前失败"); err != nil {
		t.Fatal(err)
	}
	if err = writeInstallRecord(fixture.root, "elevation.json", true); err != nil {
		t.Fatal(err)
	}
	if err = fixture.installer.recover(context.Background(), fixture.task.Next, fixture.task.Installation.Executable, true); err != nil {
		t.Fatal("精确启动确认失败", err)
	}
	for _, name := range []string{"journal.json", "error.json", "elevation.json"} {
		if _, err = fixture.root.Lstat(name); !errors.Is(err, fs.ErrNotExist) {
			t.Fatal("确认后记录未清理", name, err)
		}
	}
	if _, err = backupRoot.Lstat(backup); !errors.Is(err, fs.ErrNotExist) {
		t.Fatal("确认后备份未清理", err)
	}
	if err = fixture.installer.recover(context.Background(), fixture.task.Next, fixture.task.Installation.Executable, true); err != nil {
		t.Fatal("重复确认失败", err)
	}
}

func TestInstallRecoveryRejectsCorruptBackupAndTamperedTarget(t *testing.T) {
	for _, broken := range []string{"backup", "target", "journal", "stage_link"} {
		t.Run(broken, func(t *testing.T) {
			fixture := newInstallFixture(t)
			if err := fixture.installer.applyPrepared(context.Background(), fixture.task, fixture.prepared); err != nil {
				t.Fatal(err)
			}
			journal, _, err := readInstallJournal(fixture.root)
			if err != nil {
				t.Fatal(err)
			}
			backupRoot, backup := fixture.installer.backup(journal, fixture.parent)
			switch broken {
			case "backup":
				err = backupRoot.WriteFile(backup, []byte("损坏备份"), 0700)
			case "target":
				err = fixture.parent.WriteFile("foreign-target", []byte("损坏新程序"), 0700)
				if err == nil {
					err = platformReplaceInstall(context.Background(), fixture.parent, "foreign-target", "kitonyterms", false)
				}
			case "journal":
				err = fixture.root.WriteFile("journal.json", []byte(`{"stage":"broken"}`), 0600)
			case "stage_link":
				if err = os.RemoveAll(journal.Staging); err == nil {
					err = os.Symlink(t.TempDir(), journal.Staging)
					requireUpdateTestSymlink(t, err)
				}
			}
			if err != nil {
				t.Fatal(err)
			}
			record, err := fixture.root.ReadFile("journal.json")
			if err != nil {
				t.Fatal(err)
			}
			if broken == "target" {
				err = fixture.installer.recover(context.Background(), fixture.task.Next, fixture.task.Installation.Executable, true)
			} else {
				err = fixture.installer.rollback(context.Background())
			}
			if err == nil {
				t.Fatal("接受损坏的恢复材料")
			}
			after, readErr := fixture.root.ReadFile("journal.json")
			if readErr != nil || string(after) != string(record) {
				t.Fatal("拒绝恢复后改写了原记录", readErr)
			}
			if _, err = backupRoot.Lstat(backup); err != nil {
				t.Fatal("失败后丢失了备份", err)
			}
		})
	}
}

func TestInstallPreparationFailurePreservesOriginal(t *testing.T) {
	for _, broken := range []string{"wrong_hash", "wrong_build", "changed_source", "foreign_task", "existing_backup", "cancelled"} {
		t.Run(broken, func(t *testing.T) {
			fixture := newInstallFixture(t)
			ctx := context.Background()
			switch broken {
			case "wrong_hash":
				fixture.prepared.SHA256 = strings.Repeat("e", 64)
			case "wrong_build":
				fixture.task.Next.Commit = strings.Repeat("b", 40)
				if err := writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
					t.Fatal(err)
				}
			case "changed_source":
				fixture.task.SourceSHA256 = strings.Repeat("e", 64)
				if err := writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
					t.Fatal(err)
				}
			case "foreign_task":
				fixture.task.ParentPID++
			case "existing_backup":
				journal := installJournal{Task: fixture.task, Storage: "legacy"}
				if strings.Contains(fixture.task.Current.Target, "windows") {
					journal.Storage = "user_directory"
				}
				root, name := fixture.installer.backup(journal, fixture.parent)
				if err := root.WriteFile(name, []byte("已有备份，不得覆盖"), 0600); err != nil {
					t.Fatal(err)
				}
			case "cancelled":
				var cancel context.CancelFunc
				ctx, cancel = context.WithCancel(ctx)
				cancel()
			}
			if err := fixture.installer.applyPrepared(ctx, fixture.task, fixture.prepared); err == nil {
				t.Fatal("接受无效准备")
			}
			assertInstallOriginal(t, fixture)
			if _, err := fixture.root.Lstat("journal.json"); !errors.Is(err, fs.ErrNotExist) {
				t.Fatal("准备失败却写入可安装事务", err)
			}
			entries, err := os.ReadDir(fixture.parent.Name())
			if err != nil {
				t.Fatal(err)
			}
			for _, entry := range entries {
				if strings.HasPrefix(entry.Name(), ".kt-update-stage-") {
					t.Fatal("准备失败泄漏暂存目录")
				}
			}
		})
	}
}

func TestInstallLockAndReplacementClassification(t *testing.T) {
	root := archiveTestRoot(t)
	first, err := acquireInstallLock(context.Background(), root, "installation.lock", 0)
	if err != nil {
		t.Fatal(err)
	}
	defer first.Close()
	if _, err = acquireInstallLock(context.Background(), root, "installation.lock", 0); err == nil {
		t.Fatal("同进程重复持有安装锁")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	if _, err = acquireInstallLock(ctx, root, "installation.lock", time.Second); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal("锁等待不能取消", err)
	}
	if err = first.Close(); err != nil {
		t.Fatal(err)
	}
	second, err := acquireInstallLock(context.Background(), root, "installation.lock", 0)
	if err != nil {
		t.Fatal("锁释放后无法重新获取", err)
	}
	if err = second.Close(); err != nil {
		t.Fatal(err)
	}
	if err = (&installer{root: root}).rollback(context.Background()); err == nil {
		t.Fatal("无锁执行了恢复")
	}
	for code, want := range map[syscall.Errno]string{5: "access_denied", 32: "in_use", 33: "in_use", 1224: "in_use", 2: "other"} {
		if got := replaceErrorKind(&os.PathError{Op: "rename", Path: "target", Err: code}); got != want {
			t.Fatal("替换错误分类错误", code, got)
		}
	}
	target := filepath.Join(root.Name(), "target")
	if err = root.WriteFile("target", []byte("原程序"), 0600); err != nil {
		t.Fatal(err)
	}
	for _, failure := range []error{syscall.Errno(5), &replaceFailure{Target: target + "-other", Kind: "access_denied", Err: syscall.Errno(5)}, &replaceFailure{Target: target, Kind: "in_use", Err: syscall.Errno(32)}} {
		if replacementNeedsElevation(failure, target) {
			t.Fatal("无关错误触发提权")
		}
	}
	failure := &replaceFailure{Target: target, Kind: "access_denied", Err: syscall.Errno(5)}
	if !replacementNeedsElevation(failure, target) {
		t.Fatal("未识别目标权限错误")
	}
	if err = root.Chmod("target", 0400); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = root.Chmod("target", 0600) })
	if replacementNeedsElevation(failure, target) {
		t.Fatal("只读文件触发提权")
	}
}
