package update

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func helperInstallFixture(t *testing.T) installFixture {
	t.Helper()
	fixture := newInstallFixture(t)
	binary, err := fixture.root.ReadFile(fixture.prepared.Executable)
	if err != nil {
		t.Fatal(err)
	}
	if err = fixture.parent.WriteFile("kitonyterms", binary, 0700); err != nil {
		t.Fatal(err)
	}
	fixture.original = binary
	fixture.task.SourceSHA256 = fixture.prepared.SHA256
	fixture.task.ParentPID = uint32(os.Getpid())
	fixture.task.Package = cacheTestPackage(t, fixture.root, fixture.task.Package.Kind, []byte("只测试helper的包摘要绑定，不作为可安装归档"))
	if err = writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
		t.Fatal(err)
	}
	return fixture
}

// 测试宿主注入已知 current 身份，以独立检查进程/锁/标记协议；不验证产品CLI接线。
func runTestApplyHelper() int {
	self, err := os.Executable()
	if err != nil {
		return 2
	}
	root, err := os.OpenRoot(filepath.Dir(self))
	if err != nil {
		return 2
	}
	defer root.Close()
	task, err := readInstallTask(root)
	if err != nil {
		return 2
	}
	mode, err := root.ReadFile("test-helper-mode")
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return 2
	}
	switch string(mode) {
	case "exit":
		return 3
	case "wrong_pid", "wrong_digest", "silent":
		if string(mode) != "silent" {
			pid, digest := uint32(os.Getpid()), task.SourceSHA256
			if string(mode) == "wrong_pid" {
				pid++
			} else {
				digest = strings.Repeat("f", 64)
			}
			if err = writeInstallRecord(root, "helper-ready.json", []any{pid, digest}); err != nil {
				return 2
			}
		}
		time.Sleep(20 * time.Second)
		return 4
	}
	if err = runApplyHelper(context.Background(), root, task.Current, self, func(string) error { return errors.New("测试不得重启GUI") }); err != nil {
		return 2
	}
	return 0
}

func TestUpdateHelperHandshakeAndAbortAcrossProcesses(t *testing.T) {
	fixture := helperInstallFixture(t)
	if err := fixture.installer.installHelper(context.Background(), fixture.task); err != nil {
		t.Fatal(err)
	}
	process, err := spawnApplyHelper(context.Background(), fixture.root, fixture.task.SourceSHA256)
	if err != nil {
		t.Fatal("helper未就绪", err)
	}
	defer func() { _ = process.command.Process.Kill(); <-process.done }()
	if err = readHelperReady(fixture.root, uint32(process.command.Process.Pid), fixture.task.SourceSHA256); err != nil {
		t.Fatal(err)
	}
	if _, err = acquireInstallLock(context.Background(), fixture.root, "helper.lock", 0); err == nil {
		t.Fatal("helper未持有互斥锁")
	}
	assertInstallOriginal(t, fixture)
	if err = writeInstallRecord(fixture.root, "abort.json", true); err != nil {
		t.Fatal(err)
	}
	select {
	case <-process.done:
		if process.err != nil {
			t.Fatal("helper取消后退出失败", process.err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("helper取消未回收")
	}
	assertInstallOriginal(t, fixture)
	for _, name := range []string{"task.json", "helper-ready.json", "commit.json", "abort.json", "journal.json"} {
		if _, err = fixture.root.Lstat(name); !errors.Is(err, fs.ErrNotExist) {
			t.Fatal("取消后遗留协议标记", name, err)
		}
	}
	lock, err := acquireInstallLock(context.Background(), fixture.root, "helper.lock", 0)
	if err != nil {
		t.Fatal("helper退出后锁未释放", err)
	}
	if err = lock.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestUpdateHelperRejectsWrongHandshakeAndReapsProcess(t *testing.T) {
	for _, mode := range []string{"wrong_pid", "wrong_digest", "exit", "silent"} {
		t.Run(mode, func(t *testing.T) {
			fixture := helperInstallFixture(t)
			if err := fixture.installer.installHelper(context.Background(), fixture.task); err != nil {
				t.Fatal(err)
			}
			if err := fixture.root.WriteFile("test-helper-mode", []byte(mode), 0600); err != nil {
				t.Fatal(err)
			}
			ctx := context.Background()
			if mode == "silent" {
				var cancel context.CancelFunc
				ctx, cancel = context.WithTimeout(ctx, 200*time.Millisecond)
				defer cancel()
			}
			if process, err := spawnApplyHelper(ctx, fixture.root, fixture.task.SourceSHA256); err == nil || process != nil {
				t.Fatal("接受错误/缺失握手", err)
			}
			assertInstallOriginal(t, fixture)
			if _, err := fixture.root.Lstat("journal.json"); !errors.Is(err, fs.ErrNotExist) {
				t.Fatal("握手失败却进入了安装", err)
			}
		})
	}
}

func TestUpdateHelperRestartRequiresCommitAndParentExit(t *testing.T) {
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	root := archiveTestRoot(t)
	if err := writeInstallRecord(root, "commit.json", true); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 40*time.Millisecond)
	defer cancel()
	if approved, err := waitForUpdateRestart(ctx, root, uint32(os.Getpid()), time.Second); approved || !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal("旧进程仍在运行却放行", approved, err)
	}
	if err := writeInstallRecord(root, "abort.json", true); err != nil {
		t.Fatal(err)
	}
	if approved, err := waitForUpdateRestart(context.Background(), root, uint32(os.Getpid()), time.Second); approved || err != nil {
		t.Fatal("abort未优先", approved, err)
	}
	if err := root.Remove("abort.json"); err != nil {
		t.Fatal(err)
	}
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(executable, "--wait")
	if err = child.Start(); err != nil {
		t.Fatal(err)
	}
	pid := uint32(child.Process.Pid)
	if alive, err := updateProcessAlive(pid); err != nil || !alive {
		t.Fatal("未识别存活进程", alive, err)
	}
	if err = child.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	_ = child.Wait()
	for _, marker := range []string{"missing", "false", "null", "{}", "true false", "true"} {
		if err = root.Remove("commit.json"); err != nil && !errors.Is(err, fs.ErrNotExist) {
			t.Fatal(err)
		}
		if marker != "missing" {
			if err = root.WriteFile("commit.json", []byte(marker), 0600); err != nil {
				t.Fatal(err)
			}
		}
		approved, err := waitForUpdateRestart(context.Background(), root, pid, time.Second)
		if marker == "true" {
			if !approved || err != nil {
				t.Fatal("有效重启未放行", err)
			}
		} else if approved || err == nil {
			t.Fatal("无效commit放行", marker, err)
		}
	}
	for _, pid := range []uint32{0, 0xffffffff} {
		if _, err := updateProcessAlive(pid); err == nil {
			t.Fatal("接受非法PID", pid)
		}
	}
}

func TestUpdateHelperCopyAndIdentityBinding(t *testing.T) {
	fixture := helperInstallFixture(t)
	lease, err := acquireInstallLock(context.Background(), fixture.root, "helper.lock", 0)
	if err != nil {
		t.Fatal(err)
	}
	if err = fixture.installer.installHelper(context.Background(), fixture.task); err == nil {
		t.Fatal("覆盖了仍在运行的helper")
	}
	if err = lease.Close(); err != nil {
		t.Fatal(err)
	}
	if err = fixture.installer.installHelper(context.Background(), fixture.task); err != nil {
		t.Fatal(err)
	}
	fixture.task.ParentPID = 1
	if err = writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
		t.Fatal(err)
	}
	self := filepath.Join(fixture.root.Name(), updateHelperName())
	if _, err = validateApplyHelper(context.Background(), fixture.root, fixture.task.Current, self); err != nil {
		t.Fatal("合法helper绑定被拒绝", err)
	}
	wrong := fixture.task.Current
	wrong.Commit = strings.Repeat("f", 40)
	if _, err = validateApplyHelper(context.Background(), fixture.root, wrong, self); err == nil {
		t.Fatal("错误构建身份被接受")
	}
	if err = fixture.parent.WriteFile("kitonyterms", []byte("被替换的旧目标"), 0700); err != nil {
		t.Fatal(err)
	}
	if _, err = validateApplyHelper(context.Background(), fixture.root, fixture.task.Current, self); err == nil {
		t.Fatal("旧目标变化未拒绝")
	}
}
