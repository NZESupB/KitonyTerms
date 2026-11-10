package update

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/gofrs/flock"
)

func persistDesktopFixtureParent(t *testing.T, fixture *installFixture) {
	t.Helper()
	fixture.task.ParentPID = 0x7ffffffe
	if alive, err := updateProcessAlive(fixture.task.ParentPID); err != nil || alive {
		t.Fatal("测试退出PID不适用", err)
	}
	if err := writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
		t.Fatal(err)
	}
	if present, err := markerPresent(fixture.root, "journal.json"); err != nil {
		t.Fatal(err)
	} else if present {
		journal, _, err := readInstallJournal(fixture.root)
		if err != nil {
			t.Fatal(err)
		}
		journal.Task = fixture.task
		if err = fixture.installer.writeJournal(journal); err != nil {
			t.Fatal(err)
		}
	}
}

func openDesktopFixture(t *testing.T, fixture installFixture, current BuildInfo) *Desktop {
	t.Helper()
	if err := fixture.installer.Close(); err != nil {
		t.Fatal(err)
	}
	desktop, err := openDesktop(context.Background(), filepath.Dir(fixture.root.Name()), fixture.task.Installation.Executable, current)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := desktop.Close(); err != nil {
			t.Error(err)
		}
	})
	return desktop
}

func TestDesktopStartupHoldsLocksUntilShutdown(t *testing.T) {
	fixture := newInstallFixture(t)
	if err := fixture.root.Remove("task.json"); err != nil {
		t.Fatal(err)
	}
	desktop := openDesktopFixture(t, fixture, BuildInfo{Channel: "development", Version: "0.1.17", Target: desktopTarget()})
	if desktop.NeedsRecovery() {
		t.Fatal("普通启动误认为需要恢复")
	}
	if _, err := acquireInstallLock(context.Background(), fixture.root, "installation.lock", 0); err == nil {
		t.Fatal("GUI启动未持有安装锁")
	}
	other := flock.New(filepath.Join(filepath.Dir(fixture.root.Name()), "kitonyterms.lock"))
	defer other.Close()
	if locked, err := other.TryLock(); err != nil || locked {
		t.Fatal("既有应用锁与启动锁不兼容", locked, err)
	}
	if err := desktop.ConfirmReady(context.Background()); err != nil {
		t.Fatal("普通启动确认不幂等", err)
	}
	if err := desktop.Close(); err != nil {
		t.Fatal(err)
	}
	if locked, err := other.TryLock(); err != nil || !locked {
		t.Fatal("进程关闭后应用锁未释放", locked, err)
	}
	if err := desktop.ConfirmReady(context.Background()); err == nil {
		t.Fatal("关闭后仍能确认启动")
	}
}

func committedDesktopFixture(t *testing.T) (installFixture, installJournal) {
	t.Helper()
	fixture := newInstallFixture(t)
	persistDesktopFixtureParent(t, &fixture)
	if err := fixture.installer.applyPrepared(context.Background(), fixture.task, fixture.prepared); err != nil {
		t.Fatal(err)
	}
	journal, _, err := readInstallJournal(fixture.root)
	if err != nil {
		t.Fatal(err)
	}
	return fixture, journal
}

func TestDesktopCommittedWaitsForExplicitPageReady(t *testing.T) {
	fixture, journal := committedDesktopFixture(t)
	desktop := openDesktopFixture(t, fixture, fixture.task.Next)
	backupRoot, backup := fixture.installer.backup(journal, fixture.parent)
	if _, err := backupRoot.Lstat(backup); err != nil {
		t.Fatal("页面就绪前丢失备份", err)
	}
	if _, _, err := readInstallJournal(fixture.root); err != nil {
		t.Fatal("页面就绪前清理了journal", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := desktop.ConfirmReady(ctx); err == nil {
		t.Fatal("取消的页面确认仍然清理事务")
	}
	if _, err := backupRoot.Lstat(backup); err != nil {
		t.Fatal("取消确认丢失备份", err)
	}
	if err := desktop.ConfirmReady(context.Background()); err != nil {
		t.Fatal(err)
	}
	if present, err := markerPresent(fixture.root, "journal.json"); err != nil || present {
		t.Fatal("页面就绪后未清理事务", err)
	}
	if _, err := backupRoot.Lstat(backup); !os.IsNotExist(err) {
		t.Fatal("页面就绪后未清理备份", err)
	}
	if err := desktop.ConfirmReady(context.Background()); err != nil {
		t.Fatal("重复页面确认失败", err)
	}
}

func TestDesktopPageReadyRejectsChangedJournal(t *testing.T) {
	fixture, journal := committedDesktopFixture(t)
	desktop := openDesktopFixture(t, fixture, fixture.task.Next)
	data, err := fixture.root.ReadFile("journal.json")
	if err != nil {
		t.Fatal(err)
	}
	if err = fixture.root.WriteFile("journal.json", append(data, ' '), 0600); err != nil {
		t.Fatal(err)
	}
	if err = desktop.ConfirmReady(context.Background()); err == nil {
		t.Fatal("页面确认接受了被改写的journal")
	}
	backupRoot, backup := fixture.installer.backup(journal, fixture.parent)
	if _, err = backupRoot.Lstat(backup); err != nil {
		t.Fatal("拒绝确认丢失备份", err)
	}
}

func TestDesktopRecoveryHandshakePrecedesGUIAndKeepsLocks(t *testing.T) {
	fixture := newRelaunchFixture(t, true)
	persistDesktopFixtureParent(t, &fixture)
	desktop := openDesktopFixture(t, fixture, fixture.task.Next)
	if !desktop.NeedsRecovery() {
		t.Fatal("中断安装没有进入恢复重启")
	}
	if err := desktop.ConfirmReady(context.Background()); err == nil {
		t.Fatal("待恢复程序可以确认GUI就绪")
	}
	if err := desktop.RelaunchRecovery(context.Background()); err != nil {
		t.Fatal("启动恢复握手失败", err)
	}
	if err := restartCommitted(fixture.root); err != nil {
		t.Fatal("握手后未提交退出", err)
	}
	if _, err := acquireInstallLock(context.Background(), fixture.root, "installation.lock", 0); err == nil {
		t.Fatal("旧进程尚未结束却释放了安装锁")
	}
	if present, err := markerPresent(fixture.root, "test-restarted.json"); err != nil || present {
		t.Fatal("旧进程仍在运行却重开GUI", err)
	}
	// 测试进程必须继续，显式取消这次已握手但尚未获父退出授权的恢复。
	if err := writeInstallRecord(fixture.root, "abort.json", true); err != nil {
		t.Fatal(err)
	}
	lease, err := acquireInstallLock(context.Background(), fixture.root, "helper.lock", 5*time.Second)
	if err != nil {
		t.Fatal("恢复helper取消后未释放锁", err)
	}
	defer lease.Close()
	if _, _, err = readInstallJournal(fixture.root); err != nil {
		t.Fatal("取消启动恢复丢失journal", err)
	}
}

func TestDesktopStartupRejectsActiveForeignOrBrokenRecords(t *testing.T) {
	for _, broken := range []string{"task_parent", "relaunch_parent", "foreign_task", "foreign_journal", "invalid_journal", "helper_lock"} {
		t.Run(broken, func(t *testing.T) {
			fixture := newRelaunchFixture(t, true)
			persistDesktopFixtureParent(t, &fixture)
			var lease *installLock
			switch broken {
			case "task_parent":
				fixture.task.ParentPID = uint32(os.Getpid())
				if err := writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
					t.Fatal(err)
				}
			case "relaunch_parent":
				if err := writeInstallRecord(fixture.root, "relaunch.json", recoveryRelaunch{uint32(os.Getpid()), fixture.task.Installation.Executable, fixture.task.SourceSHA256, true}); err != nil {
					t.Fatal(err)
				}
			case "foreign_task":
				fixture.task.Installation.Executable += "-other"
				fixture.task.Installation.Target = fixture.task.Installation.Executable
				if err := writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
					t.Fatal(err)
				}
			case "foreign_journal":
				journal, _, err := readInstallJournal(fixture.root)
				if err != nil {
					t.Fatal(err)
				}
				journal.Task.Installation.Executable += "-other"
				journal.Task.Installation.Target = journal.Task.Installation.Executable
				if err = fixture.installer.writeJournal(journal); err != nil {
					t.Fatal(err)
				}
			case "invalid_journal":
				if err := fixture.root.WriteFile("journal.json", []byte(`{"stage":"broken"}`), 0600); err != nil {
					t.Fatal(err)
				}
			case "helper_lock":
				var err error
				lease, err = acquireInstallLock(context.Background(), fixture.root, "helper.lock", 0)
				if err != nil {
					t.Fatal(err)
				}
				defer lease.Close()
			}
			before, err := fixture.root.ReadFile("journal.json")
			if err != nil {
				t.Fatal(err)
			}
			if err = fixture.installer.Close(); err != nil {
				t.Fatal(err)
			}
			if desktop, openErr := openDesktop(context.Background(), filepath.Dir(fixture.root.Name()), filepath.Join(fixture.parent.Name(), "kitonyterms"), fixture.task.Next); openErr == nil || desktop != nil {
				t.Fatal("接受活跃/外来/损坏事务", openErr)
			}
			after, err := fixture.root.ReadFile("journal.json")
			if err != nil || !bytes.Equal(before, after) {
				t.Fatal("启动拒绝后改写了原记录", err)
			}
		})
	}
}

func TestDesktopCommandRejectsBeforeDataDiscovery(t *testing.T) {
	original := embeddedIdentity
	defer func() { embeddedIdentity = original }()
	build, err := json.Marshal(archiveTestBuild())
	if err != nil {
		t.Fatal(err)
	}
	for _, identity := range []string{"", base64.RawURLEncoding.EncodeToString(build)} {
		embeddedIdentity = identity
		for _, command := range []string{"--apply-update", "--relaunch-update", "--elevated-replace", "--restore-update"} {
			called := false
			handled, err := HandleDesktopCommand(context.Background(), []string{command}, io.Discard, func() (string, error) { called = true; return t.TempDir(), nil })
			if !handled || err == nil || called {
				t.Fatal("未验证helper就探测数据目录", command, err, called)
			}
		}
	}
}
