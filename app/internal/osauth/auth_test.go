package osauth

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
)

func TestGateRejectsConcurrentAndCancelledSuccess(t *testing.T) {
	started, release := make(chan struct{}), make(chan struct{})
	var calls atomic.Int32
	g := Gate{authenticate: func(context.Context, string, uintptr, string) error {
		calls.Add(1)
		close(started)
		<-release
		return nil
	}}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- g.run(ctx, "test", 1, "auto") }()
	<-started
	if err := g.run(context.Background(), "test", 1, "auto"); !errors.Is(err, ErrBusy) {
		t.Fatalf("并发请求未被阻止: %v", err)
	}
	cancel()
	close(release)
	if err := <-done; !errors.Is(err, ErrCancelled) {
		t.Fatalf("已取消认证接受迟到成功: %v", err)
	}
	if calls.Load() != 1 {
		t.Fatal("重复调用系统认证")
	}
	if err := g.run(ctx, "test", 1, "auto"); !errors.Is(err, ErrCancelled) {
		t.Fatal("已取消上下文启动认证")
	}
	if g.pending.Load() {
		t.Fatal("认证结束未释放门禁")
	}
}

func TestWindowsPolicyKeepsExplicitFailuresAndAccountIdentity(t *testing.T) {
	for _, code := range []uint32{0x80004002, 0x80040154, 0x80040111, 0x80070032} {
		if !helloUnavailable(code) {
			t.Fatalf("明确不支持未识别: %x", code)
		}
	}
	for _, code := range []uint32{0x80070005, 0x80010106, 0x80004005} {
		if helloUnavailable(code) {
			t.Fatalf("认证错误被密码回退吞掉: %x", code)
		}
	}
	if helloResult(6) != ErrCancelled || helloResult(4) != ErrBusy || helloResult(5) != ErrFailed || helloResult(0) != nil {
		t.Fatal("Hello分类错误")
	}
	cases := []struct {
		user, domain, name, currentDomain, wantUser, wantDomain string
		hasDomain                                               bool
	}{
		{"user", "", "user", "PC", "user", "PC", true},
		{`DOMAIN\user`, "", "user", "PC", "user", "DOMAIN", true},
		{"user@example.org", "", "user", "PC", "user@example.org", "", false},
		{"other", "", "user", "PC", "other", ".", true},
	}
	for _, c := range cases {
		u, d, h := accountParts(c.user, c.domain, c.name, c.currentDomain)
		if u != c.wantUser || d != c.wantDomain || h != c.hasDomain {
			t.Fatalf("账户解析错误: %q %q %v", u, d, h)
		}
	}
}
