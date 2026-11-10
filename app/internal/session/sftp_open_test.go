package session

import (
	"context"
	"errors"
	"io"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
)

func TestSFTPInitialNegotiationCancellation(t *testing.T) {
	for _, phase := range []string{"subsystem", "version"} {
		t.Run(phase, func(t *testing.T) {
			entered := make(chan struct{})
			var intercepted atomic.Bool
			server := startLoopbackSFTP(t, func(channel ssh.Channel, request *ssh.Request) bool {
				if !intercepted.CompareAndSwap(false, true) {
					return false
				}
				if phase == "version" {
					_ = request.Reply(true, nil)
				}
				close(entered)
				_, _ = io.Copy(io.Discard, channel)
				return true
			})
			manager, _ := newLoopbackManager(t, server, "tester")
			opened, err := manager.Connect("loopback")
			if err != nil {
				t.Fatal(err)
			}
			snapshot := waitConnected(t, manager, opened, true)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			done := make(chan error, 1)
			go func() {
				_, err := manager.ReadDirectory(ctx, snapshot.ID, snapshot.Generation, ".")
				done <- err
			}()
			select {
			case <-entered:
			case <-time.After(2 * time.Second):
				t.Fatal("未进入协商")
			}
			cancel()
			select {
			case err := <-done:
				if !errors.Is(err, context.Canceled) {
					t.Fatal(err)
				}
			case <-time.After(2 * time.Second):
				t.Fatal("初次协商未响应取消")
			}
			if _, err = manager.ReadDirectory(context.Background(), snapshot.ID, snapshot.Generation, "."); err != nil {
				t.Fatal("取消后未恢复 SFTP", err)
			}
			session, _ := manager.Get(snapshot.ID, snapshot.Generation)
			if session.snapshot().Status != "connected" {
				t.Fatal("正常通道取消关闭了宿主 SSH")
			}
		})
	}
}
