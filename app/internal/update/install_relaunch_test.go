package update

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func newRelaunchFixture(t *testing.T, restore bool) installFixture {
	t.Helper()
	fixture := helperInstallFixture(t)
	fixture.task.Current = archiveTestBuild()
	fixture.task.Next = archiveTestBuild()
	fixture.task.Next.Version = "1.2.4"
	fixture.task.Next.Build++
	if err := writeInstallRecord(fixture.root, "task.json", fixture.task); err != nil {
		t.Fatal(err)
	}
	if err := fixture.installer.installHelper(context.Background(), fixture.task); err != nil {
		t.Fatal(err)
	}
	if restore {
		journal := testInstallJournal(t, fixture.root)
		journal.Task, journal.Stage = fixture.task, "backed_up"
		journal.Staging = filepath.Join(fixture.parent.Name(), ".kt-update-stage-relaunch")
		backupRoot, backup := fixture.installer.backup(journal, fixture.parent)
		if err := backupRoot.WriteFile(backup, fixture.original, 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.Mkdir(journal.Staging, 0700); err != nil {
			t.Fatal(err)
		}
		if err := fixture.parent.WriteFile("kitonyterms", []byte("隔离副本中尚未恢复的新程序"), 0700); err != nil {
			t.Fatal(err)
		}
		if err := fixture.installer.writeJournal(journal); err != nil {
			t.Fatal(err)
		}
	}
	return fixture
}

func runTestRecoveryRelaunch() int {
	self, err := os.Executable()
	if err != nil {
		return 2
	}
	root, err := os.OpenRoot(filepath.Dir(self))
	if err != nil {
		return 2
	}
	defer root.Close()
	record, _, err := readRecoveryRelaunch(root)
	if err != nil {
		return 2
	}
	mode, _ := root.ReadFile("test-helper-mode")
	switch string(mode) {
	case "exit":
		return 3
	case "wrong_pid", "wrong_digest", "silent":
		if string(mode) != "silent" {
			pid, digest := uint32(os.Getpid()), record.SourceSHA256
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
	err = runRecoveryRelaunch(context.Background(), root, archiveTestBuild(), self, func(executable string) (err error) {
		// 回调替代真正的 GUI，只验证普通协调器已放开三把锁。
		for _, name := range []string{"installation.lock", "helper.lock"} {
			lock, lockErr := acquireInstallLock(context.Background(), root, name, 0)
			if lockErr != nil {
				return lockErr
			}
			if lockErr = lock.Close(); lockErr != nil {
				return lockErr
			}
		}
		data, err := os.OpenRoot(filepath.Dir(root.Name()))
		if err != nil {
			return err
		}
		defer func() { err = errors.Join(err, data.Close()) }()
		lock, err := acquireInstallLock(context.Background(), data, "kitonyterms.lock", 0)
		if err != nil {
			return err
		}
		defer func() { err = errors.Join(err, lock.Close()) }()
		return writeInstallRecord(root, "test-restarted.json", executable)
	})
	if err != nil {
		_ = writeInstallRecord(root, "test-relaunch-error.json", err.Error())
		return 2
	}
	return 0
}

func runRelaunchOldProcess(args []string) int {
	if len(args) != 1 {
		return 2
	}
	root, err := os.OpenRoot(args[0])
	if err != nil {
		return 2
	}
	defer root.Close()
	gate, err := acquireInstallLock(context.Background(), root, "installation.lock", 0)
	if err != nil {
		return 2
	}
	defer gate.Close()
	data, err := os.OpenRoot(filepath.Dir(root.Name()))
	if err != nil {
		return 2
	}
	defer data.Close()
	instance, err := acquireInstallLock(context.Background(), data, "kitonyterms.lock", 0)
	if err != nil {
		return 2
	}
	defer instance.Close()
	if err = writeInstallRecord(root, "test-parent-ready.json", uint32(os.Getpid())); err != nil {
		return 2
	}
	var input [1]byte
	_, err = os.Stdin.Read(input[:])
	if err != nil && !errors.Is(err, io.EOF) {
		return 2
	}
	return 0
}

func waitRelaunchTestFile(t *testing.T, root *os.Root, name string) {
	t.Helper()
	deadline := time.Now().Add(12 * time.Second)
	for time.Now().Before(deadline) {
		if present, err := markerPresent(root, name); err != nil {
			t.Fatal(err)
		} else if present {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("等待测试进程记录超时", name)
}

func startRelaunchTestProcess(t *testing.T, command *exec.Cmd) *updateHelperProcess {
	t.Helper()
	if err := command.Start(); err != nil {
		t.Fatal(err)
	}
	process := &updateHelperProcess{command: command, done: make(chan struct{})}
	go func() { process.err = command.Wait(); close(process.done) }()
	t.Cleanup(func() {
		_ = command.Process.Kill()
		<-process.done
	})
	return process
}

func waitRelaunchTestProcess(t *testing.T, process *updateHelperProcess, success bool, root *os.Root) {
	t.Helper()
	select {
	case <-process.done:
		if (process.err == nil) != success {
			detail, _ := root.ReadFile("test-relaunch-error.json")
			t.Fatal("恢复子进程退出状态错误", process.err, string(detail))
		}
	case <-time.After(15 * time.Second):
		t.Fatal("恢复子进程未退出")
	}
}

func TestRecoveryRelaunchStrictRustRecord(t *testing.T) {
	root := archiveTestRoot(t)
	record := recoveryRelaunch{ParentPID: 123, Executable: filepath.Join(t.TempDir(), "中文 空格", "kitonyterms"), SourceSHA256: strings.Repeat("a", 64), Restore: true}
	valid, err := json.Marshal(record)
	if err != nil {
		t.Fatal(err)
	}
	var fields []json.RawMessage
	if err = json.Unmarshal(valid, &fields); err != nil || len(fields) != 4 {
		t.Fatal("没有沿用Rust数组记录", err)
	}
	invalid := []string{"{}", "null", "[]", string(valid) + " false", "[1,2,3,4,5]"}
	for index := range fields {
		copyFields := append([]json.RawMessage(nil), fields...)
		copyFields[index] = json.RawMessage("null")
		data, _ := json.Marshal(copyFields)
		invalid = append(invalid, string(data))
	}
	for _, mutate := range []func(*recoveryRelaunch){func(r *recoveryRelaunch) { r.ParentPID = 0 }, func(r *recoveryRelaunch) { r.ParentPID = 0x80000000 }, func(r *recoveryRelaunch) { r.Executable = "relative" }, func(r *recoveryRelaunch) { r.Executable += string(os.PathSeparator) + ".." }, func(r *recoveryRelaunch) { r.SourceSHA256 = "bad" }} {
		changed := record
		mutate(&changed)
		data, _ := json.Marshal(changed)
		invalid = append(invalid, string(data))
	}
	for _, data := range invalid {
		if err = root.WriteFile("relaunch.json", []byte(data), 0600); err != nil {
			t.Fatal(err)
		}
		if _, _, err = readRecoveryRelaunch(root); err == nil {
			t.Fatal("接受无效恢复记录", data)
		}
	}
	if err = root.WriteFile("relaunch.json", valid, 0600); err != nil {
		t.Fatal(err)
	}
	actual, digest, err := readRecoveryRelaunch(root)
	if err != nil || actual != record || !validHex(digest, 64) {
		t.Fatal("旧记录读取失败", actual, digest, err)
	}
	after, _ := root.ReadFile("relaunch.json")
	if string(after) != string(valid) {
		t.Fatal("读取改写了原记录")
	}
}

func TestRecoveryRelaunchBindsHelperBuildTaskAndJournal(t *testing.T) {
	for _, restore := range []bool{false, true} {
		fixture := newRelaunchFixture(t, restore)
		prepared, err := fixture.installer.prepareRecoveryRelaunch(context.Background(), fixture.task.Installation, fixture.task.Current, restore)
		if err != nil {
			t.Fatal(err)
		}
		for _, broken := range []string{"none", "build", "helper", "digest", "executable", "self_parent", "restore"} {
			t.Run(map[bool]string{false: "reopen", true: "restore"}[restore]+"/"+broken, func(t *testing.T) {
				record := prepared
				record.ParentPID = 1
				current := fixture.task.Current
				self := filepath.Join(fixture.root.Name(), updateHelperName())
				switch broken {
				case "build":
					current.Commit = strings.Repeat("b", 40)
				case "helper":
					self = fixture.task.Installation.Executable
				case "digest":
					record.SourceSHA256 = strings.Repeat("c", 64)
				case "executable":
					record.Executable += "-foreign"
				case "self_parent":
					record.ParentPID = uint32(os.Getpid())
				case "restore":
					record.Restore = !record.Restore
				}
				if err = writeInstallRecord(fixture.root, "relaunch.json", record); err != nil {
					t.Fatal(err)
				}
				_, err = validateRecoveryRelaunch(context.Background(), fixture.root, current, self)
				if (err == nil) != (broken == "none") {
					t.Fatal("恢复身份验证不正确", err)
				}
			})
		}
	}
}

func TestRecoveryRelaunchHandshakeAbortPreservesTransaction(t *testing.T) {
	for _, restore := range []bool{false, true} {
		t.Run(map[bool]string{false: "reopen", true: "restore"}[restore], func(t *testing.T) {
			fixture := newRelaunchFixture(t, restore)
			record, err := fixture.installer.prepareRecoveryRelaunch(context.Background(), fixture.task.Installation, fixture.task.Current, restore)
			if err != nil {
				t.Fatal(err)
			}
			process, err := spawnRecoveryRelaunch(context.Background(), fixture.root, record)
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { _ = process.command.Process.Kill(); <-process.done })
			if err = writeInstallRecord(fixture.root, "abort.json", true); err != nil {
				t.Fatal(err)
			}
			waitRelaunchTestProcess(t, process, true, fixture.root)
			if _, err = fixture.root.Lstat("task.json"); err != nil {
				t.Fatal("取消恢复丢失原任务", err)
			}
			if present, err := markerPresent(fixture.root, "journal.json"); err != nil || present != restore {
				t.Fatal("取消恢复改变journal", err)
			}
			for _, name := range []string{"relaunch.json", "helper-ready.json", "commit.json", "abort.json", "test-restarted.json"} {
				if present, err := markerPresent(fixture.root, name); err != nil || present {
					t.Fatal("取消后仍有协议标记或触发重开", name, err)
				}
			}
		})
	}
}

func TestRecoveryRelaunchRejectsBadHandshakeAndCollectsChild(t *testing.T) {
	for _, mode := range []string{"wrong_pid", "wrong_digest", "exit", "silent"} {
		t.Run(mode, func(t *testing.T) {
			fixture := newRelaunchFixture(t, false)
			record, err := fixture.installer.prepareRecoveryRelaunch(context.Background(), fixture.task.Installation, fixture.task.Current, false)
			if err != nil {
				t.Fatal(err)
			}
			if err = fixture.root.WriteFile("test-helper-mode", []byte(mode), 0600); err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			if process, err := spawnRecoveryRelaunch(ctx, fixture.root, record); process != nil || err == nil {
				t.Fatal("接受错误恢复握手", err)
			}
			assertInstallOriginal(t, fixture)
		})
	}
}

func TestRecoveryRelaunchWaitsForOldProcessAndRestartsAfterUnlock(t *testing.T) {
	for _, restore := range []bool{false, true} {
		t.Run(map[bool]string{false: "reopen", true: "restore"}[restore], func(t *testing.T) {
			fixture, parent, input, process := startWaitingRecoveryRelaunch(t, restore)
			if err := writeInstallRecord(fixture.root, "commit.json", true); err != nil {
				t.Fatal(err)
			}
			select {
			case <-process.done:
				t.Fatal("父进程存活时恢复helper提前退出", process.err)
			case <-time.After(100 * time.Millisecond):
			}
			if present, err := markerPresent(fixture.root, "test-restarted.json"); err != nil || present {
				t.Fatal("父进程退出前重开应用", err)
			}
			if err := input.Close(); err != nil {
				t.Fatal(err)
			}
			waitRelaunchTestProcess(t, parent, true, fixture.root)
			waitRelaunchTestProcess(t, process, true, fixture.root)
			assertInstallOriginal(t, fixture)
			var executable string
			data, err := fixture.root.ReadFile("test-restarted.json")
			if err != nil || json.Unmarshal(data, &executable) != nil || executable != fixture.task.Installation.Executable {
				t.Fatal("没有在释放锁后重开准确的旧程序", string(data), err)
			}
			for _, name := range []string{"journal.json", "task.json", "relaunch.json", "helper-ready.json", "commit.json"} {
				if present, err := markerPresent(fixture.root, name); err != nil || present {
					t.Fatal("成功重开后遗留事务", name, err)
				}
			}
		})
	}
}

func startWaitingRecoveryRelaunch(t *testing.T, restore bool) (installFixture, *updateHelperProcess, io.WriteCloser, *updateHelperProcess) {
	t.Helper()
	fixture := newRelaunchFixture(t, restore)
	record, err := fixture.installer.prepareRecoveryRelaunch(context.Background(), fixture.task.Installation, fixture.task.Current, restore)
	if err != nil {
		t.Fatal(err)
	}
	if err = fixture.installer.Close(); err != nil {
		t.Fatal(err)
	}
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	old := exec.Command(executable, "--relaunch-old-process", fixture.root.Name())
	input, err := old.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	parent := startRelaunchTestProcess(t, old)
	waitRelaunchTestFile(t, fixture.root, "test-parent-ready.json")
	record.ParentPID = uint32(old.Process.Pid)
	if err = writeInstallRecord(fixture.root, "relaunch.json", record); err != nil {
		t.Fatal(err)
	}
	command := exec.Command(filepath.Join(fixture.root.Name(), updateHelperName()), "--relaunch-update")
	process := startRelaunchTestProcess(t, command)
	waitRelaunchTestFile(t, fixture.root, "helper-ready.json")
	if err = readHelperReady(fixture.root, uint32(command.Process.Pid), record.SourceSHA256); err != nil {
		t.Fatal(err)
	}
	return fixture, parent, input, process
}

func TestRecoveryRelaunchRejectsLateTamperingAndKeepsMaterials(t *testing.T) {
	for _, broken := range []string{"missing_commit", "backup", "journal", "record", "restored_target"} {
		t.Run(broken, func(t *testing.T) {
			restore := broken != "restored_target"
			fixture, parent, input, process := startWaitingRecoveryRelaunch(t, restore)
			switch broken {
			case "backup":
				journal, _, err := readInstallJournal(fixture.root)
				if err != nil {
					t.Fatal(err)
				}
				backupRoot, backup := fixture.installer.backup(journal, fixture.parent)
				if err = backupRoot.WriteFile(backup, []byte("损坏备份"), 0700); err != nil {
					t.Fatal(err)
				}
			case "journal", "record":
				name := "journal.json"
				if broken == "record" {
					name = "relaunch.json"
				}
				data, err := fixture.root.ReadFile(name)
				if err != nil {
					t.Fatal(err)
				}
				if err = fixture.root.WriteFile(name, append(data, ' '), 0600); err != nil {
					t.Fatal(err)
				}
			case "restored_target":
				if err := fixture.parent.WriteFile("kitonyterms", []byte("等待期间被改变的目标"), 0700); err != nil {
					t.Fatal(err)
				}
			}
			if broken != "missing_commit" {
				if err := writeInstallRecord(fixture.root, "commit.json", true); err != nil {
					t.Fatal(err)
				}
			}
			if err := input.Close(); err != nil {
				t.Fatal(err)
			}
			waitRelaunchTestProcess(t, parent, true, fixture.root)
			waitRelaunchTestProcess(t, process, false, fixture.root)
			if present, err := markerPresent(fixture.root, "test-restarted.json"); err != nil || present {
				t.Fatal("拒绝恢复后仍重开了程序", err)
			}
			if _, err := fixture.root.Lstat("relaunch.json"); errors.Is(err, fs.ErrNotExist) {
				t.Fatal("失败丢失恢复记录")
			}
			if restore {
				journal, _, err := readInstallJournal(fixture.root)
				if err != nil {
					t.Fatal("失败丢失journal", err)
				}
				backupRoot, backup := fixture.installer.backup(journal, fixture.parent)
				if _, err = backupRoot.Lstat(backup); err != nil {
					t.Fatal("失败丢失备份", err)
				}
				if _, err = os.Stat(journal.Staging); err != nil {
					t.Fatal("失败丢失暂存目录", err)
				}
			}
		})
	}
}
