package externaledit

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sync"
	"testing"
	"time"

	"kitonyterms/internal/model"
)

type fakeRemote struct {
	mu           sync.Mutex
	data         []byte
	failure      error
	writes       int
	afterWrite   func()
	blockRead    bool
	disconnected bool
}

func (f *fakeRemote) EditSessionActive(string, string) bool {
	f.mu.Lock()
	defer f.mu.Unlock()
	return !f.disconnected
}

func (f *fakeRemote) ReadEditFile(ctx context.Context, _, _, _ string) ([]byte, string, error) {
	f.mu.Lock()
	block := f.blockRead
	data := bytes.Clone(f.data)
	f.mu.Unlock()
	if block {
		<-ctx.Done()
		return nil, "", ctx.Err()
	}
	return data, hash(data), nil
}
func (f *fakeRemote) WriteEditFile(_ context.Context, _, _, _ string, data []byte, expected string) (string, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.writes++
	if f.failure != nil {
		return "", f.failure
	}
	if hash(f.data) != expected {
		return "", fmt.Errorf("远端冲突")
	}
	f.data = bytes.Clone(data)
	if f.afterWrite != nil {
		f.afterWrite()
	}
	return hash(data), nil
}
func setup(t *testing.T, f *fakeRemote) *Manager {
	t.Helper()
	m, err := New(t.Context(), t.TempDir(), f, func(string, string) error { return nil })
	if err != nil {
		t.Fatal(err)
	}
	m.interval = 10 * time.Millisecond
	t.Cleanup(m.Close)
	return m
}
func waitEdit(t *testing.T, m *Manager, id string, predicate func(model.ExternalEdit) bool) model.ExternalEdit {
	t.Helper()
	end := time.Now().Add(3 * time.Second)
	for time.Now().Before(end) {
		for _, s := range m.List() {
			if s.ID == id && predicate(s) {
				return s
			}
		}
		time.Sleep(time.Millisecond)
	}
	t.Fatalf("编辑状态未收敛: %+v", m.List())
	return model.ExternalEdit{}
}
func start(t *testing.T, m *Manager) model.ExternalEdit {
	t.Helper()
	s, err := m.Start("host", "generation", "/srv/a '中.bin", "")
	if err != nil {
		t.Fatal(err)
	}
	return waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool { return s.Status == "watching" })
}
func save(t *testing.T, file string, data []byte) {
	t.Helper()
	temp := file + ".save"
	if err := os.WriteFile(temp, data, 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(temp, file); err != nil {
		t.Fatal(err)
	}
}
func TestPrivateCopyScopedOnceAndCleanup(t *testing.T) {
	f := &fakeRemote{data: []byte{0, 255, 1}}
	m := setup(t, f)
	s := start(t, m)
	for name, mode := range map[string]os.FileMode{s.LocalPath: 0600, filepath.Dir(s.LocalPath): 0700} {
		info, err := os.Stat(name)
		if err != nil {
			t.Fatal(err)
		}
		if runtime.GOOS != "windows" && info.Mode().Perm() != mode {
			t.Fatal("权限不私有", info.Mode())
		}
	}
	if _, err := m.Start("host", "generation", s.RemotePath, ""); err != nil || len(m.List()) != 1 {
		t.Fatal("重复打开未复用", err)
	}
	if err := m.Action(t.Context(), "host", "old", s.ID, "once"); err == nil {
		t.Fatal("旧代次操作被接受")
	}
	data := []byte{0, 254, 3, 4}
	save(t, s.LocalPath, data)
	waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool { return s.Status == "changed" && s.Dirty })
	if err := m.Action(t.Context(), s.SessionID, s.Generation, s.ID, "once"); err != nil {
		t.Fatal(err)
	}
	end := time.Now().Add(3 * time.Second)
	for len(m.List()) > 0 && time.Now().Before(end) {
		time.Sleep(time.Millisecond)
	}
	if len(m.List()) != 0 {
		t.Fatal(m.List())
	}
	if _, err := os.Stat(s.LocalPath); !os.IsNotExist(err) {
		t.Fatal("成功副本未清理", err)
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.writes != 1 || !bytes.Equal(f.data, data) {
		t.Fatal("内容或上传次数错误")
	}
}
func TestAutoFailureRequiresExplicitRetryAndKeepsCopy(t *testing.T) {
	f := &fakeRemote{data: []byte("old"), failure: fmt.Errorf("拒绝上传")}
	m := setup(t, f)
	s := start(t, m)
	save(t, s.LocalPath, []byte("first"))
	if err := m.Action(t.Context(), s.SessionID, s.Generation, s.ID, "auto"); err != nil {
		t.Fatal(err)
	}
	waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool { return s.Status == "error" && !s.AutoUpload })
	time.Sleep(70 * time.Millisecond)
	f.mu.Lock()
	writes := f.writes
	f.failure = nil
	f.mu.Unlock()
	if writes != 1 {
		t.Fatal("失败后重复上传", writes)
	}
	if err := m.Action(t.Context(), s.SessionID, s.Generation, s.ID, "auto"); err != nil {
		t.Fatal(err)
	}
	waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool { return s.Status == "watching" && s.AutoUpload && !s.Dirty })
	save(t, s.LocalPath, []byte("second"))
	waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool {
		f.mu.Lock()
		defer f.mu.Unlock()
		return s.Status == "watching" && !s.Dirty && string(f.data) == "second"
	})
	f.mu.Lock()
	got := string(f.data)
	f.mu.Unlock()
	if got != "second" {
		t.Fatal("自动保存未回传", got)
	}
	save(t, s.LocalPath, []byte("retained"))
	m.StopSession(s.SessionID, s.Generation)
	retained := waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool { return s.Status == "retained" && s.Dirty })
	data, err := os.ReadFile(retained.LocalPath)
	if err != nil || string(data) != "retained" {
		t.Fatal("修改未保留", err)
	}
	if m.Active() != 0 {
		t.Fatal("停止后仍活动")
	}
	if err = m.Action(t.Context(), s.SessionID, s.Generation, s.ID, "dismiss"); err != nil {
		t.Fatal(err)
	}
	if _, err = os.Stat(retained.LocalPath); err != nil {
		t.Fatal("关闭记录删除了保留副本", err)
	}
}
func TestSaveDuringUploadStaysPending(t *testing.T) {
	f := &fakeRemote{data: []byte("old")}
	m := setup(t, f)
	s := start(t, m)
	save(t, s.LocalPath, []byte("first"))
	f.mu.Lock()
	f.afterWrite = func() {
		if err := os.WriteFile(s.LocalPath, []byte("newer"), 0600); err != nil {
			panic(err)
		}
	}
	f.mu.Unlock()
	if err := m.Action(t.Context(), s.SessionID, s.Generation, s.ID, "once"); err != nil {
		t.Fatal(err)
	}
	state := waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool { return s.Status == "changed" && s.Dirty && s.Error != "" })
	if state.AutoUpload {
		t.Fatal("单次上传改变模式")
	}
	m.Close()
	if _, err := os.Stat(s.LocalPath); err != nil {
		t.Fatal("上传期间的新保存被删除", err)
	}
}
func TestCancelledDownloadAndFailureRemainVisible(t *testing.T) {
	f := &fakeRemote{blockRead: true}
	m := setup(t, f)
	s, err := m.Start("host", "generation", "/file", "")
	if err != nil {
		t.Fatal(err)
	}
	if err = m.Action(t.Context(), s.SessionID, s.Generation, s.ID, "stop"); err != nil {
		t.Fatal(err)
	}
	if m.Active() != 0 {
		t.Fatal("下载取消未结束")
	}
	m.Close()
	if _, err = m.Start("h", "g", "/file", ""); err == nil {
		t.Fatal("关闭后接受新任务")
	}
}
func TestSymlinkCannotReadOutsideCopy(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("此项需Windows符号链接权限")
	}
	f := &fakeRemote{data: []byte("old")}
	m := setup(t, f)
	s := start(t, m)
	outside := filepath.Join(t.TempDir(), "private")
	if err := os.WriteFile(outside, []byte("must-not-upload"), 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.Remove(s.LocalPath); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, s.LocalPath); err != nil {
		t.Fatal(err)
	}
	if err := m.Action(t.Context(), s.SessionID, s.Generation, s.ID, "once"); err != nil {
		t.Fatal(err)
	}
	waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool { return s.Status == "error" })
	m.Close()
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.writes != 0 {
		t.Fatal("上传了目录外内容")
	}
	info, err := os.Stat(outside)
	if err != nil || info.Mode().Perm() != 0644 {
		t.Fatal("目录外文件被修改", err)
	}
}
func TestCommandTemplatePreservesLiteralFileArgument(t *testing.T) {
	file := "/tmp/目录 ' ; $(touch forbidden).txt"
	for _, tc := range []struct {
		input, program string
		args           []string
	}{{`code --wait "{file}"`, "code", []string{"--wait", file}}, {`"/Applications/Visual Studio Code.app/bin/code" --reuse-window`, "/Applications/Visual Studio Code.app/bin/code", []string{"--reuse-window", file}}, {`editor '' --file={file}`, "editor", []string{"", "--file=" + file}}} {
		program, args, err := Command(tc.input, file)
		if err != nil || program != tc.program || !reflect.DeepEqual(args, tc.args) {
			t.Fatal(program, args, err)
		}
	}
	for _, input := range []string{"", `code "open`, `code trailing\`, `{file}`, "a\x00b"} {
		if _, _, err := Command(input, file); err == nil {
			t.Fatalf("接受无效模板: %q", input)
		}
	}
	settings := model.DefaultSettings()
	settings.DefaultEditor = "custom --file {file}"
	settings.Editors = []model.Editor{{Name: "custom", Command: settings.DefaultEditor}}
	if command, err := Resolve(settings, ""); err != nil || command != settings.DefaultEditor {
		t.Fatal(command, err)
	}
	if _, err := Resolve(settings, "unlisted {file}"); err == nil {
		t.Fatal("接受任意未配置命令")
	}
}

func TestNaturalDisconnectStopsWatchingAndKeepsDirtyFile(t *testing.T) {
	f := &fakeRemote{data: []byte("old")}
	m := setup(t, f)
	s := start(t, m)
	save(t, s.LocalPath, []byte("unsent"))
	f.mu.Lock()
	f.disconnected = true
	f.mu.Unlock()
	waitEdit(t, m, s.ID, func(s model.ExternalEdit) bool { return s.Status == "retained" && s.Dirty })
	if m.Active() != 0 {
		t.Fatal("自然断开后监听仍在运行")
	}
	if value, err := os.ReadFile(s.LocalPath); err != nil || string(value) != "unsent" {
		t.Fatal("未上传副本丢失", err)
	}
}
