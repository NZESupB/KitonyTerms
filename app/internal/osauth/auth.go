package osauth

import (
	"context"
	"errors"
	"sync/atomic"
	"time"
)

var (
	ErrCancelled        = errors.New("系统认证已取消")
	ErrUnavailable      = errors.New("此平台无法查看已保存的凭据")
	ErrFailed           = errors.New("系统认证失败")
	ErrWrongCredentials = errors.New("账户密码错误或与当前用户不匹配")
	ErrRestricted       = errors.New("系统限制了该账户的登录")
	ErrLocked           = errors.New("系统账户已锁定")
	ErrBusy             = errors.New("已有系统认证正在进行")
)

type Gate struct {
	pending      atomic.Bool
	authenticate func(context.Context, string, uintptr, string) error
}

var native = Gate{authenticate: authenticatePlatform}

// Authenticate只完成一次系统验证；不缓存成功结果，也不接收SSH凭据。
func Authenticate(ctx context.Context, reason string, owner uintptr, method string) error {
	return native.run(ctx, reason, owner, method)
}

func (g *Gate) run(ctx context.Context, reason string, owner uintptr, method string) error {
	if method != "auto" && method != "account_password" {
		return ErrFailed
	}
	if err := ctx.Err(); err != nil {
		return ErrCancelled
	}
	if !g.pending.CompareAndSwap(false, true) {
		return ErrBusy
	}
	defer g.pending.Store(false)
	ctx, cancel := context.WithTimeout(ctx, 120*time.Second)
	defer cancel()
	err := g.authenticate(ctx, reason, owner, method)
	if ctx.Err() != nil {
		return ErrCancelled
	}
	return err
}

func Failure(err error) string {
	switch {
	case err == nil:
		return ""
	case errors.Is(err, ErrCancelled), errors.Is(err, context.Canceled), errors.Is(err, context.DeadlineExceeded):
		return "cancelled"
	case errors.Is(err, ErrUnavailable):
		return "unavailable"
	case errors.Is(err, ErrBusy):
		return "busy"
	case errors.Is(err, ErrWrongCredentials):
		return "wrong_credentials"
	case errors.Is(err, ErrRestricted):
		return "restricted"
	case errors.Is(err, ErrLocked):
		return "locked"
	default:
		return "failed"
	}
}
