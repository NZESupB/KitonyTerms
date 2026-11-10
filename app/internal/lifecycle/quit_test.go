package lifecycle

import (
	"sync"
	"sync/atomic"
	"testing"
)

func TestQuitCancellationIdentityAndCleanupGate(t *testing.T) {
	var notices atomic.Int32
	var control *Controller
	control = New(func() {
		notices.Add(1)
		if control.Snapshot().RequestID == "" {
			t.Error("通知先于退出请求")
		}
	})
	if !control.ShouldQuit() {
		t.Fatal("服务启动前阻止退出")
	}
	control.Enable()
	if control.ShouldQuit() {
		t.Fatal("未确认退出即放行")
	}
	first := control.Snapshot().RequestID
	if first == "" || notices.Load() != 1 {
		t.Fatal("未生成退出请求")
	}
	if control.ShouldQuit() || control.Snapshot().RequestID != first {
		t.Fatal("重复关闭生成了其它请求")
	}
	if control.Complete(first) {
		t.Fatal("尚未收尾就放行")
	}
	if err := control.Cancel(first); err != nil {
		t.Fatal(err)
	}
	if control.Snapshot().RequestID != "" {
		t.Fatal("取消后请求仍然存在")
	}
	control.ShouldQuit()
	second := control.Snapshot().RequestID
	if second == first {
		t.Fatal("取消后复用了请求 ID")
	}
	if err := control.Cancel(first); err == nil {
		t.Fatal("旧取消影响新请求")
	}
	if _, err := control.Begin(first); err == nil {
		t.Fatal("旧确认影响新请求")
	}
	if begin, err := control.Begin(second); err != nil || !begin {
		t.Fatal("确认失败", err)
	}
	if control.ShouldQuit() {
		t.Fatal("清理完成前退出")
	}
	if err := control.Cancel(second); err == nil {
		t.Fatal("清理中允许取消")
	}
	if !control.Complete(second) || !control.ShouldQuit() {
		t.Fatal("清理后未放行")
	}
}

func TestQuitConcurrentRequestsAndSingleCleanup(t *testing.T) {
	control := New(nil)
	control.Enable()
	var group sync.WaitGroup
	for range 64 {
		group.Go(func() {
			if control.ShouldQuit() {
				t.Error("并发请求提前放行")
			}
		})
	}
	group.Wait()
	id := control.Snapshot().RequestID
	if id == "" {
		t.Fatal("未生成退出请求")
	}
	var begins atomic.Int32
	for range 64 {
		group.Go(func() {
			begin, err := control.Begin(id)
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
		t.Fatalf("清理启动 %d 次", begins.Load())
	}
	if control.Complete("stale") {
		t.Fatal("旧请求放行退出")
	}
	if !control.Complete(id) || !control.ShouldQuit() {
		t.Fatal("完成后退出仍受阻")
	}
}
