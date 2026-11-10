package session

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

func TestSFTPDirectoryActionsAndBatchDownload(t *testing.T) {
	server := startLoopback(t)
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	session := waitConnected(t, manager, opened, true)
	ctx := context.Background()
	remote := filepath.Join(server.root, "目录")
	if err = manager.CreateDirectory(ctx, session.ID, session.Generation, remoteTestPath(remote)); err != nil {
		t.Fatal(err)
	}
	if err = manager.CreateDirectory(ctx, session.ID, session.Generation, remoteTestPath(filepath.Join(remote, "空目录"))); err != nil {
		t.Fatal(err)
	}
	before := filepath.Join(remote, "原名.txt")
	after := filepath.Join(remote, "重命名.txt")
	if err = os.WriteFile(before, []byte("中文 batch\n"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = manager.RenamePath(ctx, session.ID, session.Generation, remoteTestPath(before), remoteTestPath(after)); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(before, []byte("保留"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = manager.RenamePath(ctx, session.ID, session.Generation, remoteTestPath(before), remoteTestPath(after)); err == nil {
		t.Fatal("同名目标被覆盖")
	}
	destination := t.TempDir()
	summary, err := manager.DownloadBatch(ctx, session.ID, session.Generation, []string{remoteTestPath(remote)}, destination)
	if err != nil || summary.Files != 2 || summary.Bytes != "19" {
		t.Fatalf("批量下载失败: %+v %v", summary, err)
	}
	data, err := os.ReadFile(filepath.Join(destination, "目录", "重命名.txt"))
	if err != nil || string(data) != "中文 batch\n" {
		t.Fatal("文件字节不一致", err)
	}
	if info, err := os.Stat(filepath.Join(destination, "目录", "空目录")); err != nil || !info.IsDir() {
		t.Fatal("空目录未保留")
	}
	if _, err = manager.DownloadBatch(ctx, session.ID, session.Generation, []string{remoteTestPath(remote)}, destination); err == nil {
		t.Fatal("已有本地目录被合并或覆盖")
	}
	if err = manager.RemovePaths(ctx, session.ID, session.Generation, []string{remoteTestPath(remote)}); err != nil {
		t.Fatal(err)
	}
	if _, err = os.Stat(remote); !os.IsNotExist(err) {
		t.Fatal("递归删除未完成")
	}
	if err = manager.RemovePaths(ctx, session.ID, session.Generation, []string{"/"}); err == nil {
		t.Fatal("接受了根目录删除")
	}
}

func TestSFTPBatchPreflightAndCancellation(t *testing.T) {
	server := startLoopback(t)
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	session := waitConnected(t, manager, opened, true)
	ctx := context.Background()
	remote := filepath.Join(server.root, "root")
	if err = os.Mkdir(remote, 0700); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(remote, "A"), []byte("one"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(remote, "a"), []byte("two"), 0600); err != nil {
		t.Fatal(err)
	}
	destination := t.TempDir()
	// macOS 默认卷可能不区分大小写，显式用两个不同根同名验证冲突。
	second := filepath.Join(server.root, "other")
	if err = os.Mkdir(second, 0700); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(second, "A"), []byte("three"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err = manager.DownloadBatch(ctx, session.ID, session.Generation, []string{remoteTestPath(filepath.Join(remote, "A")), remoteTestPath(filepath.Join(second, "A"))}, destination); err == nil {
		t.Fatal("同名批次被接受")
	}
	entries, err := os.ReadDir(destination)
	if err != nil || len(entries) != 0 {
		t.Fatal("预检失败后仍写入本地")
	}
	if err = os.Symlink(filepath.Join(remote, "A"), filepath.Join(remote, "link")); err != nil {
		t.Fatal(err)
	}
	if _, err = manager.DownloadBatch(ctx, session.ID, session.Generation, []string{remoteTestPath(remote)}, destination); err == nil {
		t.Fatal("链接未拒绝")
	}
	entries, _ = os.ReadDir(destination)
	if len(entries) != 0 {
		t.Fatal("发现链接前已下载部分项目")
	}
	cancelled, cancel := context.WithCancel(ctx)
	cancel()
	if _, err = manager.DownloadBatch(cancelled, session.ID, session.Generation, []string{remoteTestPath(filepath.Join(remote, "A"))}, destination); err == nil {
		t.Fatal("取消未生效")
	}
	entries, _ = os.ReadDir(destination)
	if len(entries) != 0 {
		t.Fatal("取消残留本地文件")
	}
}

func TestPortableDownloadNames(t *testing.T) {
	for _, name := range []string{"..", "a/b", "a\\b", "NUL.txt", "COM1", "LPT3.log", "a:", "trailing.", "trailing ", "line\n"} {
		if portableName(name) {
			t.Fatalf("接受了不可移植名称 %q", name)
		}
	}
	for _, name := range []string{"中文.txt", "normal", "COM10", "a b.txt"} {
		if !portableName(name) {
			t.Fatalf("拒绝了有效名称 %q", name)
		}
	}
}
