package session

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestFileLockWaitCancellation(t *testing.T) {
	var lock fileMutex
	if err := lock.LockContext(context.Background()); err != nil {
		t.Fatal(err)
	}
	defer lock.Unlock()
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	done := make(chan error, 1)
	go func() { done <- lock.LockContext(ctx) }()
	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("文件锁等待未响应取消")
	}
}

func TestSFTPCancellationReopensOnlyFileChannel(t *testing.T) {
	server := startLoopback(t)
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	snapshot := waitConnected(t, manager, opened, true)
	session, err := manager.Get(snapshot.ID, snapshot.Generation)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	first, release, err := session.fileScope(ctx)
	if err != nil {
		t.Fatal(err)
	}
	cancel()
	release()
	second, release, err := session.fileScope(context.Background())
	if err != nil {
		t.Fatal("取消后不能重建 SFTP", err)
	}
	if first == second {
		t.Fatal("复用了已取消的文件通道")
	}
	release()
	if _, err = manager.ReadDirectory(context.Background(), snapshot.ID, snapshot.Generation, remoteTestPath(server.root)); err != nil {
		t.Fatal(err)
	}
	cancelled, cancelOperation := context.WithCancel(context.Background())
	cancelOperation()
	if _, err = manager.RunOperation(cancelled, snapshot.ID, snapshot.Generation, "status", ""); !errors.Is(err, context.Canceled) {
		t.Fatal("运维取消未传递", err)
	}
	if session.snapshot().Status != "connected" {
		t.Fatal("文件或 exec 取消关闭了宿主终端")
	}
}

func TestTemporaryCleanupAfterCancelledChannel(t *testing.T) {
	server := startLoopback(t)
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	snapshot := waitConnected(t, manager, opened, true)
	session, _ := manager.Get(snapshot.ID, snapshot.Generation)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	client, release, err := session.fileScope(ctx)
	if err != nil {
		t.Fatal(err)
	}
	temporary := filepath.Join(server.root, ".kitony-upload-"+randomID())
	file, err := client.OpenFile(remoteTestPath(temporary), os.O_WRONLY|os.O_CREATE|os.O_EXCL)
	if err != nil {
		release()
		t.Fatal(err)
	}
	if err = file.Close(); err != nil {
		release()
		t.Fatal(err)
	}
	cancel()
	release()
	if err = session.cleanupTemporary(ctx, client, remoteTestPath(temporary)); err != nil {
		t.Fatal(err)
	}
	if _, err = os.Stat(temporary); !os.IsNotExist(err) {
		t.Fatal("取消后的临时文件未回收", err)
	}
	if _, err = manager.ReadDirectory(context.Background(), snapshot.ID, snapshot.Generation, remoteTestPath(server.root)); err != nil {
		t.Fatal("临时清理关闭了宿主 SSH", err)
	}
}
