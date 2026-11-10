package session

import (
	"context"
	"testing"
	"time"

	"kitonyterms/internal/model"
)

func TestActivityTracksTransferAndShutdown(t *testing.T) {
	server := startLoopback(t)
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	session := waitConnected(t, manager, opened, true)
	started := make(chan struct{})
	_, err = manager.startTransfer(session.ID, session.Generation, "upload", func(ctx context.Context) (model.TransferSummary, error) {
		close(started)
		<-ctx.Done()
		return model.TransferSummary{}, ctx.Err()
	})
	if err != nil {
		t.Fatal(err)
	}
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("任务未启动")
	}
	activity := manager.Activity()
	if activity.Sessions != 1 || activity.Transfers != 1 || activity.Operations != 0 || activity.ContainerTerminals != 0 {
		t.Fatalf("在途活动缺失: %+v", activity)
	}
	manager.Shutdown()
	if activity = manager.Activity(); activity != (model.AppActivity{}) {
		t.Fatalf("退出后活动残留: %+v", activity)
	}
	if _, err = manager.Connect("loopback"); err == nil {
		t.Fatal("退出后仍接受连接")
	}
}
