package update

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"testing"
	"time"
)

// 这些命令只编译进测试二进制，模拟与 GUI 无关的独立持锁/崩溃进程。
func runInstallLockProbe(args []string) int {
	if len(args) != 1 {
		return 2
	}
	root, err := os.OpenRoot(args[0])
	if err != nil {
		return 2
	}
	defer root.Close()
	_, err = acquireInstallLock(context.Background(), root, "installation.lock", 0)
	result := "locked"
	if err != nil {
		result = "busy"
	}
	_, _ = os.Stdout.WriteString(result)
	// 故意由 os.Exit 回收锁句柄，覆盖进程退出后的系统释放行为。
	return 0
}

func runInstallCrashProbe(args []string) int {
	if len(args) != 2 {
		return 2
	}
	root, err := os.OpenRoot(args[0])
	if err != nil {
		return 2
	}
	defer root.Close()
	engine, err := openInstaller(context.Background(), root)
	if err != nil {
		return 2
	}
	defer engine.Close()
	task, err := readInstallTask(root)
	if err != nil {
		return 2
	}
	digest, err := hashInstallFile(context.Background(), root, ".payload-fixture/kitonyterms")
	if err != nil {
		return 2
	}
	engine.checkpoint = func(stage string) error {
		if stage == args[1] {
			os.Exit(86)
		}
		return nil
	}
	if err = engine.applyPrepared(context.Background(), task, preparedPayload{Directory: ".payload-fixture", Payload: ".payload-fixture/kitonyterms", SHA256: digest}); err != nil {
		return 2
	}
	return 3
}

func TestInstallLockAcrossProcessesAndExit(t *testing.T) {
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	root := archiveTestRoot(t)
	lock, err := acquireInstallLock(context.Background(), root, "installation.lock", 0)
	if err != nil {
		t.Fatal(err)
	}
	defer lock.Close()
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"busy", "locked", "locked"} {
		output, err := runUpdateTool(context.Background(), 10*time.Second, 1024, executable, "--install-lock-probe", root.Name())
		if err != nil || string(output) != want {
			t.Fatal("跨进程锁不正确", want, string(output), err)
		}
		if want == "busy" {
			if err = lock.Close(); err != nil {
				t.Fatal(err)
			}
		}
	}
}

func TestInstallRecoveryAfterProcessCrash(t *testing.T) {
	for _, point := range []string{"prepared", "backed_up", "replaced", "committed"} {
		t.Run(point, func(t *testing.T) {
			fixture := newInstallFixture(t)
			if err := fixture.installer.Close(); err != nil {
				t.Fatal(err)
			}
			executable, err := os.Executable()
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
			defer cancel()
			err = exec.CommandContext(ctx, executable, "--install-crash-probe", fixture.root.Name(), point).Run()
			var exited *exec.ExitError
			if !errors.As(err, &exited) || exited.ExitCode() != 86 {
				t.Fatal("没有在指定阶段模拟崩溃", err)
			}
			engine, err := openInstaller(context.Background(), fixture.root)
			if err != nil {
				t.Fatal("进程崩溃后锁未释放", err)
			}
			defer engine.Close()
			if err = engine.recover(context.Background(), fixture.task.Current, fixture.task.Installation.Executable, false); err != nil {
				t.Fatal("崩溃后恢复失败", err)
			}
			assertInstallOriginal(t, fixture)
		})
	}
}
