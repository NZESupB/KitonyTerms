package session

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"kitonyterms/internal/model"
)

func awaitTransfer(t *testing.T, manager *Manager, session model.SessionSnapshot, id string) model.TransferProgress {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		all, err := manager.Transfers(session.ID, session.Generation)
		if err != nil {
			t.Fatal(err)
		}
		for _, value := range all {
			if value.ID == id && terminalTransfer(value.Status) {
				return value
			}
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("传输没有收敛终态")
	return model.TransferProgress{}
}
func TestUploadDirectoryOverwriteAndProgress(t *testing.T) {
	server := startLoopback(t)
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	session := waitConnected(t, manager, opened, true)
	dir := filepath.Join(t.TempDir(), "目录")
	if err = os.MkdirAll(filepath.Join(dir, "empty"), 0700); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(dir, "sample.txt"), []byte("hello 中文"), 0640); err != nil {
		t.Fatal(err)
	}
	started, err := manager.StartUpload(session.ID, session.Generation, remoteTestPath(server.root), []string{dir}, false)
	if err != nil {
		t.Fatal(err)
	}
	result := awaitTransfer(t, manager, session, started.ID)
	if result.Status != "succeeded" || result.Files != 1 || result.TotalFiles != 1 || result.Bytes != "12" || result.TotalBytes != "12" {
		t.Fatalf("%+v", result)
	}
	remote := filepath.Join(server.root, "目录", "sample.txt")
	if content, err := os.ReadFile(remote); err != nil || string(content) != "hello 中文" {
		t.Fatal(string(content), err)
	}
	if info, err := os.Stat(filepath.Join(server.root, "目录", "empty")); err != nil || !info.IsDir() {
		t.Fatal("未保留空目录", err)
	}
	if err = os.Chmod(remote, 0600); err != nil {
		t.Fatal(err)
	}
	originalInfo, err := os.Stat(remote)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(dir, "sample.txt"), []byte("changed"), 0640); err != nil {
		t.Fatal(err)
	}
	if _, err = manager.UploadBatch(context.Background(), session.ID, session.Generation, remoteTestPath(server.root), []string{dir}, false); err == nil {
		t.Fatal("默认覆盖了已有目标")
	}
	if _, err = manager.UploadBatch(context.Background(), session.ID, session.Generation, remoteTestPath(server.root), []string{dir}, true); err != nil {
		t.Fatal(err)
	}
	if content, err := os.ReadFile(remote); err != nil || string(content) != "changed" {
		t.Fatal(string(content), err)
	}
	if info, err := os.Stat(remote); err != nil || info.Mode().Perm() != originalInfo.Mode().Perm() {
		t.Fatal("覆盖未保留权限", err)
	}
	downloaded, err := manager.StartDownload(session.ID, session.Generation, []string{remoteTestPath(filepath.Dir(remote))}, t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if result = awaitTransfer(t, manager, session, downloaded.ID); result.Status != "succeeded" || result.Bytes != "7" || result.Files != 1 {
		t.Fatalf("%+v", result)
	}
	if _, err = manager.Transfers(session.ID, "stale"); err == nil {
		t.Fatal("旧代次读取了任务")
	}
}
func TestUploadPreflightRejectsSymlinkAndBatchConflicts(t *testing.T) {
	server := startLoopback(t)
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	session := waitConnected(t, manager, opened, true)
	first := filepath.Join(t.TempDir(), "same")
	second := filepath.Join(t.TempDir(), "same")
	for _, local := range []string{first, second} {
		if err = os.WriteFile(local, []byte("data"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	if _, err = manager.UploadBatch(context.Background(), session.ID, session.Generation, remoteTestPath(server.root), []string{first, second}, false); err == nil {
		t.Fatal("未拒绝批次名称冲突")
	}
	link := filepath.Join(t.TempDir(), "link")
	if err = os.Symlink(first, link); err != nil {
		t.Fatal(err)
	}
	if _, err = manager.UploadBatch(context.Background(), session.ID, session.Generation, remoteTestPath(server.root), []string{first, link}, true); err == nil {
		t.Fatal("未拒绝链接")
	}
	entries, err := os.ReadDir(server.root)
	if err != nil || len(entries) != 0 {
		t.Fatal("预检失败仍修改了远端", err)
	}
}
func TestTransferCancellationWhileWaitingAndRecovery(t *testing.T) {
	server := startLoopback(t)
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	session := waitConnected(t, manager, opened, true)
	s, _ := manager.Get(session.ID, session.Generation)
	if err = s.fileMu.LockContext(context.Background()); err != nil {
		t.Fatal(err)
	}
	local := filepath.Join(t.TempDir(), "queued.txt")
	if err = os.WriteFile(local, []byte(strings.Repeat("x", 1024)), 0600); err != nil {
		t.Fatal(err)
	}
	started, err := manager.StartUpload(session.ID, session.Generation, remoteTestPath(server.root), []string{local}, false)
	if err != nil {
		s.fileMu.Unlock()
		t.Fatal(err)
	}
	_, busyErr := manager.StartUpload(session.ID, session.Generation, remoteTestPath(server.root), []string{local}, false)
	cancelErr := manager.CancelTransfer(session.ID, session.Generation, started.ID)
	s.fileMu.Unlock()
	if busyErr == nil || cancelErr != nil {
		t.Fatal("任务限流或取消失效", busyErr, cancelErr)
	}
	if result := awaitTransfer(t, manager, session, started.ID); result.Status != "cancelled" {
		t.Fatalf("%+v", result)
	}
	if _, err = os.Stat(filepath.Join(server.root, "queued.txt")); !os.IsNotExist(err) {
		t.Fatal("已取消任务仍提交", err)
	}
	started, err = manager.StartUpload(session.ID, session.Generation, remoteTestPath(server.root), []string{local}, false)
	if err != nil {
		t.Fatal(err)
	}
	if result := awaitTransfer(t, manager, session, started.ID); result.Status != "succeeded" {
		t.Fatalf("%+v", result)
	}
}
