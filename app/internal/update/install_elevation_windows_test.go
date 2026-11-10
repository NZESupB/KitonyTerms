//go:build windows

package update

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

func TestWindowsUpdatePinnedInputsAndLog(t *testing.T) {
	directory := filepath.Join(t.TempDir(), "事务目录")
	if err := os.Mkdir(directory, 0700); err != nil {
		t.Fatal(err)
	}
	source := filepath.Join(directory, "payload")
	if err := os.WriteFile(source, []byte("verified"), 0600); err != nil {
		t.Fatal(err)
	}
	pins, err := pinUpdateDirectories(directory)
	if err != nil {
		t.Fatal(err)
	}
	defer closeUpdatePins(pins)
	input, err := pinUpdatePath(source, false)
	if err != nil {
		t.Fatal(err)
	}
	defer input.Close()
	data, err := os.ReadFile(source)
	if err != nil || string(data) != "verified" {
		t.Fatal("固定后不能读取", err)
	}
	if os.WriteFile(source, []byte("changed"), 0600) == nil || os.Remove(source) == nil {
		t.Fatal("固定输入可被改写或删除")
	}
	if os.Rename(directory, directory+"-moved") == nil {
		t.Fatal("固定父目录可被替换")
	}
	if err = input.Close(); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(source, []byte("changed"), 0600); err != nil {
		t.Fatal("释放固定后无法写入", err)
	}
	log := filepath.Join(directory, "update-install.log")
	if err = os.WriteFile(log, nil, 0600); err != nil {
		t.Fatal(err)
	}
	output, err := pinUpdateHandle(log, false, true)
	if err != nil {
		t.Fatal(err)
	}
	defer output.Close()
	if err = os.WriteFile(log, []byte("安装日志"), 0600); err != nil {
		t.Fatal("固定日志不能写入", err)
	}
	if os.Remove(log) == nil {
		t.Fatal("固定日志可被删除")
	}
	if os.Rename(log, log+"-changed") == nil {
		t.Fatal("固定日志可被换名")
	}
	if err = output.Close(); err != nil {
		t.Fatal(err)
	}
	if err = os.Remove(log); err != nil {
		t.Fatal("释放固定后无法删除日志", err)
	}
}

func TestWindowsUpdatePinRejectsHardLinks(t *testing.T) {
	directory := t.TempDir()
	source := filepath.Join(directory, "source")
	if err := os.WriteFile(source, []byte("verified"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.Link(source, filepath.Join(directory, "alias")); err != nil {
		t.Fatal(err)
	}
	if file, err := pinUpdatePath(source, false); err == nil {
		_ = file.Close()
		t.Fatal("固定了硬链接输入")
	}
}

func TestWindowsUpdatePinRejectsReparseDirectories(t *testing.T) {
	base, outside := t.TempDir(), t.TempDir()
	link := filepath.Join(base, "junction")
	requireUpdateTestSymlink(t, os.Symlink(outside, link))
	if pins, err := pinUpdateDirectories(link); err == nil {
		_ = closeUpdatePins(pins)
		t.Fatal("固定了重解析目录")
	}
}

func TestWindowsShellExecuteInfoLayout(t *testing.T) {
	var info updateShellExecuteInfo
	if unsafe.Sizeof(info) != 112 || unsafe.Offsetof(info.Process) != 104 {
		t.Fatal("Windows amd64 ShellExecuteExW结构布局错误")
	}
}

func TestWindowsCanonicalTransactionPathForms(t *testing.T) {
	root := archiveTestRoot(t)
	canonical, err := canonicalUpdatePath(root.Name())
	if err != nil {
		t.Fatal(err)
	}
	ordinary, err := windowsExternalPath(canonical)
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{ordinary, canonical} {
		actual, err := canonicalUpdatePath(path)
		if err != nil || actual != canonical {
			t.Fatal("普通与扩展事务路径不一致", err)
		}
	}
	journal := testInstallJournal(t, root)
	journal.Storage = "user_directory"
	for _, form := range []string{ordinary, canonical} {
		journal.Staging = filepath.Join(form, ".kt-update-stage-compatible")
		if err = journal.validate(root); err != nil {
			t.Fatal("不同路径形式的旧journal无法校验", err)
		}
	}
	file := filepath.Join(ordinary, "same-executable.exe")
	if err = os.WriteFile(file, []byte("old"), 0600); err != nil {
		t.Fatal(err)
	}
	if !sameInstallPath(file, filepath.Join(canonical, "same-executable.exe")) {
		t.Fatal("普通与扩展目标路径不等价")
	}
	journal.Task.Installation.Target = file
	journal.Task.Installation.Executable = file
	if err = journal.validate(root); err != nil {
		t.Fatal(err)
	}
}

func TestWindowsUpdatePinnedDirectoryWithoutChildren(t *testing.T) {
	for _, operation := range []string{"rename", "remove"} {
		t.Run(operation, func(t *testing.T) {
			directory := filepath.Join(t.TempDir(), "独立空目录")
			if err := os.Mkdir(directory, 0700); err != nil {
				t.Fatal(err)
			}
			pins, err := pinUpdateDirectories(directory)
			if err != nil {
				t.Fatal(err)
			}
			change := func() error {
				if operation == "rename" {
					return os.Rename(directory, directory+"-moved")
				}
				return os.Remove(directory)
			}
			blocked := change()
			if err = closeUpdatePins(pins); err != nil {
				t.Fatal(err)
			}
			if blocked == nil {
				t.Fatal("仅固定目录时仍可改变路径")
			}
			if err = change(); err != nil {
				t.Fatal("释放固定后无法改变目录", err)
			}
		})
	}
}

func TestWindowsUpdateToolRunsExactExtensionlessPE(t *testing.T) {
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	binary, err := os.ReadFile(self)
	if err != nil {
		t.Fatal(err)
	}
	directory := t.TempDir()
	payload := filepath.Join(directory, "payload")
	if err = os.WriteFile(payload, binary, 0700); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(payload+".exe", []byte("不得选择邻近程序"), 0700); err != nil {
		t.Fatal(err)
	}
	canonical, err := canonicalUpdatePath(directory)
	if err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{payload, filepath.Join(canonical, "payload")} {
		data, err := runUpdateTool(context.Background(), 5*time.Second, 1<<20, path, "--overflow")
		if err != nil || len(data) != 1<<20 {
			t.Fatal("未执行显式无扩展名载荷", err)
		}
	}
	missing := filepath.Join(directory, "missing")
	if err = os.WriteFile(missing+".exe", binary, 0700); err != nil {
		t.Fatal(err)
	}
	if _, err = runUpdateTool(context.Background(), 5*time.Second, 1<<20, missing, "--overflow"); err == nil {
		t.Fatal("显式目标不存在时执行了邻近程序")
	}
}

func TestWindowsUpdateCleanupPreservesReplacedEntry(t *testing.T) {
	root := archiveTestRoot(t)
	if err := root.WriteFile("entry", []byte("original"), 0600); err != nil {
		t.Fatal(err)
	}
	file, err := root.Open("entry")
	if err != nil {
		t.Fatal(err)
	}
	expected, statErr := file.Stat()
	closeErr := file.Close()
	if statErr != nil || closeErr != nil {
		t.Fatal(statErr, closeErr)
	}
	if err = root.Rename("entry", "retained"); err != nil {
		t.Fatal(err)
	}
	if err = root.WriteFile("entry", []byte("foreign"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = platformRemoveInstallEntry(root, "entry", expected); err == nil {
		t.Fatal("回收了身份已改变的路径")
	}
	for name, want := range map[string]string{"entry": "foreign", "retained": "original"} {
		data, err := root.ReadFile(name)
		if err != nil || string(data) != want {
			t.Fatal("路径身份拒绝后材料未保持", err)
		}
	}
}

func TestWindowsUpdateReplaceWaitsForAccessRelease(t *testing.T) {
	for _, mode := range []string{"released", "readonly", "canceled"} {
		t.Run(mode, func(t *testing.T) {
			root := archiveTestRoot(t)
			for name, content := range map[string]string{"source": "next", "target": "original"} {
				if err := root.WriteFile(name, []byte(content), 0600); err != nil {
					t.Fatal(err)
				}
			}
			if err := root.Chmod("target", 0400); err != nil {
				t.Fatal(err)
			}
			defer root.Chmod("target", 0600)
			if err := root.Rename("source", "target"); !errors.Is(err, windows.ERROR_ACCESS_DENIED) {
				t.Fatal("夹具未产生真实访问拒绝", err)
			}
			ctx := context.Background()
			if mode == "canceled" {
				var cancel context.CancelFunc
				ctx, cancel = context.WithTimeout(ctx, 50*time.Millisecond)
				defer cancel()
			}
			var released chan error
			if mode == "released" {
				released = make(chan error, 1)
				go func() {
					time.Sleep(200 * time.Millisecond)
					released <- root.Chmod("target", 0600)
				}()
			}
			started := time.Now()
			err := platformReplaceInstall(ctx, root, "source", "target", false)
			if released != nil {
				if releaseErr := <-released; releaseErr != nil {
					t.Fatal(releaseErr)
				}
			}
			want := "original"
			switch mode {
			case "released":
				if err != nil || time.Since(started) < 150*time.Millisecond {
					t.Fatal("没有等到访问恢复", err)
				}
				want = "next"
			case "readonly":
				if !errors.Is(err, windows.ERROR_ACCESS_DENIED) || replacementNeedsElevation(err, filepath.Join(root.Name(), "target")) || time.Since(started) < 2900*time.Millisecond {
					t.Fatal("永久只读错误未保留或误判为提权", err)
				}
			case "canceled":
				if !errors.Is(err, context.DeadlineExceeded) || time.Since(started) > time.Second {
					t.Fatal("等待访问拒绝不能及时取消", err)
				}
			}
			data, readErr := root.ReadFile("target")
			if readErr != nil || string(data) != want {
				t.Fatal("替换结果与终态不一致", readErr)
			}
			if mode != "released" {
				data, readErr = root.ReadFile("source")
				if readErr != nil || string(data) != "next" {
					t.Fatal("失败或取消后源材料丢失", readErr)
				}
			}
		})
	}
}
