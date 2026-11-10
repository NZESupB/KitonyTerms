package lifecycle

import (
	"context"
	"errors"
	"sync"
	"sync/atomic"
	"testing"
)

func TestInstallIntentAdmissionAbortAndStaleOwnership(t *testing.T) {
	c := New(nil)
	c.Enable()
	done, err := c.BeginOperation()
	if err != nil {
		t.Fatal(err)
	}
	if _, err = c.StartInstall(context.Background()); err == nil {
		t.Fatal("在途工作未阻止安装")
	}
	if c.Snapshot().ProtectedOperations != 1 {
		t.Fatal("活动元数据错误")
	}
	done()
	done()
	r, err := c.StartInstall(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if c.ShouldQuit() || c.Snapshot().RequestID != r.ID || c.Snapshot().Intent != "install" {
		t.Fatal("普通关闭未复用安装请求")
	}
	if _, err = c.BeginOperation(); err == nil {
		t.Fatal("安装期间仍接受受保护工作")
	}
	if _, err = c.Begin(r.ID); err == nil {
		t.Fatal("普通退出接口接管安装")
	}
	if _, err = c.BeginInstallCommit(r.ID); err == nil || c.Complete(r.ID) {
		t.Fatal("辅助程序未就绪就放行退出")
	}
	cancelled, err := c.BeginInstallAbort(r.ID)
	if err != nil || cancelled != r || !errors.Is(r.Context.Err(), context.Canceled) {
		t.Fatal("取消未传播到准备上下文", err)
	}
	if c.ReadyInstall(r.ID) {
		t.Fatal("取消后仍接受准备就绪")
	}
	select {
	case <-r.done:
		t.Fatal("取消被误当成准备已经结束")
	default:
	}
	r.Prepared()
	r.Prepared()
	r.WaitPrepared()
	if !c.FailInstall(r.ID) {
		t.Fatal("回收后没有恢复运行")
	}
	next, err := c.StartInstall(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer next.Prepared()
	if next.ID == r.ID || c.FailInstall(r.ID) {
		t.Fatal("旧请求清除了新安装")
	}
	if _, err = c.BeginInstallAbort(r.ID); err == nil {
		t.Fatal("旧取消影响了新安装")
	}
	c.FailInstall(next.ID)
	if done, err = c.BeginOperation(); err != nil {
		t.Fatal("失败后未恢复业务准入", err)
	}
	done()
}

func TestInstallSingleCommitAndFailureRecovery(t *testing.T) {
	c := New(nil)
	c.Enable()
	r, err := c.StartInstall(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer r.Prepared()
	if !c.ReadyInstall(r.ID) {
		t.Fatal("就绪失败")
	}
	var group sync.WaitGroup
	var begins atomic.Int32
	for range 64 {
		group.Go(func() {
			begin, err := c.BeginInstallCommit(r.ID)
			if err != nil {
				t.Error(err)
			}
			if begin {
				begins.Add(1)
			}
		})
	}
	group.Wait()
	if begins.Load() != 1 {
		t.Fatal("重复执行安装提交", begins.Load())
	}
	if _, err = c.BeginInstallAbort(r.ID); err == nil {
		t.Fatal("提交期间允许页面取消")
	}
	if !c.FailInstall(r.ID) {
		t.Fatal("提交失败后没有恢复准入")
	}
	next, err := c.StartInstall(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer next.Prepared()
	if !c.ReadyInstall(next.ID) {
		t.Fatal("新准备未就绪")
	}
	if begin, err := c.BeginInstallCommit(next.ID); err != nil || !begin {
		t.Fatal(err)
	}
	if err = c.AuthorizeInstall(next.ID, func() error { return nil }); err != nil {
		t.Fatal(err)
	}
	if c.Complete(next.ID) || !c.ShouldQuit() || c.FailInstall(next.ID) {
		t.Fatal("提交完成后的退出归属错误")
	}
}

func TestInstallAndProtectedOperationAdmissionAreAtomic(t *testing.T) {
	for range 200 {
		c := New(nil)
		c.Enable()
		start := make(chan struct{})
		var group sync.WaitGroup
		var r *InstallRequest
		var done func()
		group.Go(func() { <-start; r, _ = c.StartInstall(context.Background()) })
		group.Go(func() { <-start; done, _ = c.BeginOperation() })
		close(start)
		group.Wait()
		if (r == nil) == (done == nil) {
			t.Fatal("安装与受保护工作被同时接受或同时拒绝")
		}
		if r != nil {
			r.Prepared()
			c.FailInstall(r.ID)
		} else {
			done()
		}
	}
}

func TestOrdinaryQuitWaitsForProtectedOperation(t *testing.T) {
	c := New(nil)
	c.Enable()
	done, err := c.BeginOperation()
	if err != nil {
		t.Fatal(err)
	}
	c.ShouldQuit()
	id := c.Snapshot().RequestID
	if _, err = c.Begin(id); err == nil {
		t.Fatal("选择器或同步尚未结束就开始永久关闭")
	}
	done()
	if begin, err := c.Begin(id); err != nil || !begin {
		t.Fatal("活动完成后仍无法退出", err)
	}
	if !c.Complete(id) {
		t.Fatal("普通退出未放行")
	}
}

func TestInstallAuthorizationFailureKeepsExitBlocked(t *testing.T) {
	c := New(nil)
	c.Enable()
	r, err := c.StartInstall(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	defer r.Prepared()
	if !c.ReadyInstall(r.ID) {
		t.Fatal("准备未就绪")
	}
	if _, err = c.BeginInstallCommit(r.ID); err != nil {
		t.Fatal(err)
	}
	injected := errors.New("提交落盘失败")
	if err = c.AuthorizeInstall(r.ID, func() error { return injected }); !errors.Is(err, injected) {
		t.Fatal("提交失败未返回根因", err)
	}
	if c.ShouldQuit() {
		t.Fatal("提交失败后放行了退出")
	}
	if !c.FailInstall(r.ID) {
		t.Fatal("提交失败不能恢复业务准入")
	}
	done, err := c.BeginOperation()
	if err != nil {
		t.Fatal(err)
	}
	done()
}
