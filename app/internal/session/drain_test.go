package session

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"kitonyterms/internal/model"
)

func drainTransferFixture(t *testing.T) (*Manager, model.SessionSnapshot, func()) {
	t.Helper()
	m, _ := newLoopbackManager(t, startLoopback(t), "tester")
	opened, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	s := waitConnected(t, m, opened, true)
	started, finish := make(chan struct{}), make(chan struct{})
	var once sync.Once
	release := func() { once.Do(func() { close(finish) }) }
	t.Cleanup(release)
	_, err = m.startTransfer(s.ID, s.Generation, "upload", func(ctx context.Context) (model.TransferSummary, error) {
		close(started)
		<-ctx.Done()
		<-finish
		return model.TransferSummary{}, ctx.Err()
	})
	if err != nil {
		t.Fatal(err)
	}
	select {
	case <-started:
	case <-time.After(time.Second):
		t.Fatal("传输未启动")
	}
	return m, s, release
}

func TestDrainWaitsForRemovedSessionWorkersAndOwnsAdmission(t *testing.T) {
	m, s, finish := drainTransferFixture(t)
	if err := m.CloseSession(s.ID, s.Generation); err != nil {
		t.Fatal(err)
	}
	if m.Activity() != (model.AppActivity{}) {
		t.Fatal("测试会话未从活动列表移除")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	type result struct {
		release func()
		err     error
	}
	done := make(chan result, 1)
	go func() { release, err := m.Drain(ctx); done <- result{release, err} }()
	deadline := time.Now().Add(time.Second)
	for {
		m.mu.Lock()
		draining := m.draining != ""
		m.mu.Unlock()
		if draining {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("未关闭新任务准入")
		}
		time.Sleep(time.Millisecond)
	}
	select {
	case <-done:
		t.Fatal("活动数归零被误当成工作已经结束")
	default:
	}
	if _, err := m.Connect("loopback"); err == nil {
		t.Fatal("清理中仍接受连接")
	}
	if _, err := m.Drain(ctx); err == nil {
		t.Fatal("重叠清理取得了相同准入门")
	}
	finish()
	var drained result
	select {
	case drained = <-done:
	case <-ctx.Done():
		t.Fatal("实际工作结束后仍未完成清理")
	}
	if drained.err != nil {
		t.Fatal(drained.err)
	}
	if _, err := m.Connect("loopback"); err == nil {
		t.Fatal("清理完成却在安装提交前自动开放准入")
	}
	drained.release()
	next, err := m.Connect("loopback")
	if err != nil {
		t.Fatal("安装取消后不能重新连接", err)
	}
	waitConnected(t, m, next, true)
	release, err := m.Drain(ctx)
	if err != nil {
		t.Fatal(err)
	}
	drained.release()
	if _, err := m.Connect("loopback"); err == nil {
		t.Fatal("旧释放函数打开了新清理的准入门")
	}
	release()
}

func TestDrainTimeoutRestoresAdmissionWithoutEndingManager(t *testing.T) {
	m, _, finish := drainTransferFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	if release, err := m.Drain(ctx); !errors.Is(err, context.DeadlineExceeded) || release != nil {
		t.Fatal("清理超时未显式失败", err)
	}
	next, err := m.Connect("loopback")
	if err != nil {
		t.Fatal("超时后未恢复连接准入", err)
	}
	waitConnected(t, m, next, true)
	finish()
	m.Shutdown()
	if _, err = m.Connect("loopback"); err == nil {
		t.Fatal("永久关闭后准入重新开放")
	}
}

func TestDrainRejectsAllWorkerEntryPoints(t *testing.T) {
	m, s, finish := drainTransferFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	_, _ = m.Drain(ctx)
	finish()
	release, err := m.Drain(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	if _, err = m.StartUpload(s.ID, s.Generation, "/tmp", []string{"unused"}, false); err == nil {
		t.Fatal("清理期间接受传输")
	}
	if _, err = m.PollMonitor(s.ID, s.Generation, randomID()); err == nil {
		t.Fatal("清理期间接受监控")
	}
	if _, err = m.StartContainerTerminal(s.ID, s.Generation, model.ContainerTerminalRequest{ExecID: randomID(), Container: "abcdef123456", Cols: 80, Rows: 24}); err == nil {
		t.Fatal("清理期间接受容器终端")
	}
	reply, err := m.ExecuteOperation(context.Background(), s.ID, s.Generation, model.OperationRequest{ID: randomID(), Kind: "capabilities"})
	if err == nil && reply.Error == nil {
		t.Fatal("清理期间接受运维请求")
	}
}
