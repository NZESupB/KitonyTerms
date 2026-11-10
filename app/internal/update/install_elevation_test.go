package update

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestWindowsExternalPathsAndArguments(t *testing.T) {
	for source, expected := range map[string]string{
		`\\?\C:\Program Files\中文\kitonyterms.exe`: `C:\Program Files\中文\kitonyterms.exe`,
		`C:\Users\中文 用户\updates\setup.exe`:        `C:\Users\中文 用户\updates\setup.exe`,
		`\\?\UNC\server\share\中文 用户\setup.exe`:    `\\server\share\中文 用户\setup.exe`,
		`\\server\share\setup.exe`:                `\\server\share\setup.exe`,
		`C:\`:                                     `C:\`,
	} {
		actual, err := windowsExternalPath(source)
		if err != nil || actual != expected {
			t.Fatalf("路径转换错误：%q %q %v", source, actual, err)
		}
	}
	for _, path := range []string{`relative\setup.exe`, `C:setup.exe`, `\\.\C:\setup.exe`, `\\?\GLOBALROOT\Device\setup.exe`, `C:\folder.\setup.exe`, `C:\NUL\setup.exe`, `C:\COM¹.txt\setup.exe`, `C:\a\..\setup.exe`, `C:\a\\setup.exe`, `\\?\UNC\server\share\folder.\setup.exe`, `\\server`, `C:\a` + "\x00" + `\setup.exe`, `C:\` + strings.Repeat("目录", 140) + `\setup.exe`} {
		if _, err := windowsExternalPath(path); err == nil {
			t.Fatal("接受危险外部路径", path)
		}
	}
	for input, expected := range map[string]string{"": `""`, `C:\用户 目录\`: `"C:\用户 目录\\"`, `a"b`: `"a\"b"`, `a\"b`: `"a\\\"b"`} {
		actual, err := windowsCommandArguments([]string{input})
		if err != nil || actual != expected {
			t.Fatalf("参数引用错误：%q %q %q %v", input, actual, expected, err)
		}
	}
	if _, err := windowsCommandArguments([]string{"a\x00b"}); err == nil {
		t.Fatal("接受NUL参数")
	}
	args, err := windowsInstallerArguments(`\\?\C:\Program Files\中文`, `\\?\D:\原用户 更新\update-install.log`)
	if err != nil {
		t.Fatal(err)
	}
	if len(args) != 8 || args[6] != `/DIR=C:\Program Files\中文` || args[7] != `/LOG=D:\原用户 更新\update-install.log` {
		t.Fatal("安装目录或日志未固定", args)
	}
	if args[4] != "/NOCLOSEAPPLICATIONS" || args[5] != "/NORESTARTAPPLICATIONS" {
		t.Fatal("安装器可以自行关闭或重启程序")
	}
}

func TestElevationArgumentsAndState(t *testing.T) {
	root := t.TempDir()
	digest := strings.Repeat("a", 64)
	for _, operation := range []elevationOperation{elevationReplace, elevationRestore} {
		actual, actualRoot, actualDigest, err := parseElevationArguments([]string{operation.argument(), root, digest})
		if err != nil || actual != operation || actualRoot != root || actualDigest != digest {
			t.Fatal("丢失原用户事务身份", err)
		}
	}
	for _, args := range [][]string{nil, {"--restore-update", root}, {"--other", root, digest}, {"--elevated-replace", "relative", digest}, {"--elevated-replace", root, "bad"}, {"--elevated-replace", root, digest, "extra"}} {
		if _, _, _, err := parseElevationArguments(args); err == nil {
			t.Fatal("接受无效管理员参数", args)
		}
	}
	base := installJournal{Stage: "backed_up", StagedSHA256: digest, Task: installTask{Elevate: true}}
	if err := checkElevationState(base, elevationReplace); err != nil {
		t.Fatal(err)
	}
	for _, mutate := range []func(*installJournal){func(j *installJournal) { j.Task.Elevate = false }, func(j *installJournal) { j.Task.Installation.Installed = true }, func(j *installJournal) { j.Task.Installation.Bundle = true }, func(j *installJournal) { j.Stage = "prepared" }, func(j *installJournal) { j.Stage = "committed" }, func(j *installJournal) { j.StagedSHA256 = "" }} {
		journal := base
		mutate(&journal)
		if err := checkElevationState(journal, elevationReplace); err == nil {
			t.Fatal("接受非法提权替换状态", journal)
		}
	}
	for _, stage := range []string{"backed_up", "committed"} {
		base.Stage = stage
		if err := checkElevationState(base, elevationRestore); err != nil {
			t.Fatal(err)
		}
	}
	base.Stage = "prepared"
	if checkElevationState(base, elevationRestore) == nil || checkElevationState(base, "arbitrary") == nil {
		t.Fatal("接受非法恢复操作")
	}
}

func TestElevatedOutcomeIsBoundToJournalAndOperation(t *testing.T) {
	root := archiveTestRoot(t)
	digest := strings.Repeat("a", 64)
	if err := readElevatedOutcome(root, elevationReplace, digest); err == nil {
		t.Fatal("缺少结果却成功")
	}
	for _, content := range []string{
		`{"operation":"restore","journal_sha256":"` + digest + `","error":null}`,
		`{"operation":"replace","journal_sha256":"` + strings.Repeat("b", 64) + `","error":null}`,
		`{"operation":"replace","journal_sha256":"` + digest + `"}`,
		`{"operation":"replace","operation":"replace","journal_sha256":"` + digest + `","error":null}`,
		`{"operation":"replace","journal_sha256":"` + digest + `","error":"模拟管理员拒绝"}`,
	} {
		if err := root.WriteFile("elevated-result.json", []byte(content), 0600); err != nil {
			t.Fatal(err)
		}
		if err := readElevatedOutcome(root, elevationReplace, digest); err == nil {
			t.Fatal("接受过期或损坏结果", content)
		}
	}
	if err := writeInstallRecord(root, "elevated-result.json", elevatedOutcome{Operation: elevationReplace, JournalSHA256: digest}); err != nil {
		t.Fatal(err)
	}
	if err := readElevatedOutcome(root, elevationReplace, digest); err != nil {
		t.Fatal(err)
	}
}

func TestElevatedWaitRetainsOwnershipUntilActualExit(t *testing.T) {
	for _, reason := range []string{"timeout", "cancel", "wait_error"} {
		t.Run(reason, func(t *testing.T) {
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			budget := time.Hour
			if reason == "timeout" {
				budget = time.Millisecond
			}
			if reason == "cancel" {
				cancel()
			}
			var exited atomic.Bool
			ready := make(chan struct{})
			done := make(chan error, 1)
			injected := errors.New("模拟等待API错误")
			go func() {
				first := true
				done <- waitElevatedExit(ctx, budget, func() (bool, uint32, error) {
					if first {
						close(ready)
						first = false
					}
					if exited.Load() {
						return true, 0, nil
					}
					if reason == "wait_error" {
						return false, 259, injected
					}
					return false, 259, nil
				})
			}()
			<-ready
			select {
			case err := <-done:
				t.Fatal("子进程存活时放行恢复", err)
			case <-time.After(80 * time.Millisecond):
			}
			exited.Store(true)
			select {
			case err := <-done:
				want := error(context.DeadlineExceeded)
				if reason == "cancel" {
					want = context.Canceled
				}
				if reason == "wait_error" {
					want = injected
				}
				if !errors.Is(err, want) {
					t.Fatal("未保留等待失败原因", err)
				}
			case <-time.After(time.Second):
				t.Fatal("实际退出后仍未收敛")
			}
		})
	}
	if err := waitElevatedExit(context.Background(), time.Second, func() (bool, uint32, error) { return true, 7, nil }); err == nil {
		t.Fatal("非零退出成功")
	}
	if err := waitElevatedExit(context.Background(), time.Second, func() (bool, uint32, error) { return true, 0, nil }); err != nil {
		t.Fatal(err)
	}
}

func TestElevatedValidationRejectsForeignBuildAndHelper(t *testing.T) {
	fixture := newInstallFixture(t)
	if err := fixture.installer.applyPrepared(context.Background(), fixture.task, fixture.prepared); err != nil {
		t.Fatal(err)
	}
	journal, _, err := readInstallJournal(fixture.root)
	if err != nil {
		t.Fatal(err)
	}
	if err = fixture.root.WriteFile(updateHelperName(), fixture.original, 0700); err != nil {
		t.Fatal(err)
	}
	self := filepath.Join(fixture.root.Name(), updateHelperName())
	if err = validateElevatedRequest(context.Background(), fixture.root, journal, fixture.task.Current, self, elevationRestore); err != nil {
		t.Fatal(err)
	}
	wrong := fixture.task.Current
	wrong.Commit = strings.Repeat("f", 40)
	if validateElevatedRequest(context.Background(), fixture.root, journal, wrong, self, elevationRestore) == nil {
		t.Fatal("接受其他构建")
	}
	if err = fixture.root.WriteFile(updateHelperName(), []byte("changed"), 0700); err != nil {
		t.Fatal(err)
	}
	if validateElevatedRequest(context.Background(), fixture.root, journal, fixture.task.Current, self, elevationRestore) == nil {
		t.Fatal("接受被更换的helper")
	}
	if err = os.WriteFile(filepath.Join(fixture.root.Name(), "foreign"), fixture.original, 0700); err != nil {
		t.Fatal(err)
	}
	if validateElevatedRequest(context.Background(), fixture.root, journal, fixture.task.Current, filepath.Join(fixture.root.Name(), "foreign"), elevationRestore) == nil {
		t.Fatal("接受其他位置的helper")
	}
}

func TestElevatedChildWaitKeepsInstallationLock(t *testing.T) {
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	root := archiveTestRoot(t)
	gate, err := acquireInstallLock(context.Background(), root, "installation.lock", 0)
	if err != nil {
		t.Fatal(err)
	}
	defer gate.Close()
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	child := exec.Command(executable, "--wait")
	if err = child.Start(); err != nil {
		t.Fatal(err)
	}
	childDone := make(chan struct{})
	go func() { _ = child.Wait(); close(childDone) }()
	t.Cleanup(func() { _ = child.Process.Kill(); <-childDone })
	returned := make(chan error, 1)
	go func() {
		returned <- waitElevatedExit(context.Background(), time.Millisecond, func() (bool, uint32, error) {
			select {
			case <-childDone:
				return true, uint32(child.ProcessState.ExitCode()), nil
			default:
				return false, 259, nil
			}
		})
	}()
	select {
	case err = <-returned:
		t.Fatal("真实子进程退出前已结束等待", err)
	case <-time.After(80 * time.Millisecond):
	}
	if other, lockErr := acquireInstallLock(context.Background(), root, "installation.lock", 0); lockErr == nil {
		_ = other.Close()
		t.Fatal("等待期间安装锁已释放")
	}
	if err = child.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	select {
	case err = <-returned:
		if !errors.Is(err, context.DeadlineExceeded) {
			t.Fatal("未保留超时原因", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("真实子进程退出后未回收")
	}
	if err = gate.Close(); err != nil {
		t.Fatal(err)
	}
	next, err := acquireInstallLock(context.Background(), root, "installation.lock", 0)
	if err != nil {
		t.Fatal("实际退出并释放后不能重新取得锁", err)
	}
	if err = next.Close(); err != nil {
		t.Fatal(err)
	}
}
