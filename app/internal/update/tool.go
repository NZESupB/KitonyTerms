package update

import (
	"bytes"
	"context"
	"fmt"
	"sync"
	"time"
)

type toolOutput struct {
	mu       sync.Mutex
	buffer   bytes.Buffer
	limit    int
	cancel   context.CancelFunc
	overflow bool
}

func (o *toolOutput) Write(data []byte) (int, error) {
	o.mu.Lock()
	defer o.mu.Unlock()
	if len(data) > o.limit-o.buffer.Len() {
		o.overflow = true
		o.cancel()
		return 0, fmt.Errorf("更新工具输出超过上限")
	}
	return o.buffer.Write(data)
}

func runUpdateTool(ctx context.Context, budget time.Duration, outputLimit int, executable string, arguments ...string) ([]byte, error) {
	deadline, stopDeadline := context.WithTimeout(ctx, budget)
	defer stopDeadline()
	call, cancel := context.WithCancel(deadline)
	defer cancel()
	stdout := &toolOutput{limit: outputLimit, cancel: cancel}
	stderr := &toolOutput{limit: 64 << 10, cancel: cancel}
	err := executeUpdateTool(call, executable, arguments, stdout, stderr)
	if stdout.overflow || stderr.overflow {
		return nil, fmt.Errorf("更新工具输出超过上限")
	}
	if deadline.Err() != nil {
		return nil, deadline.Err()
	}
	if err != nil {
		return nil, fmt.Errorf("更新工具执行失败：%w", err)
	}
	return append([]byte(nil), stdout.buffer.Bytes()...), nil
}

func verifyBuildCommand(ctx context.Context, executable string, expected BuildInfo) error {
	data, err := runUpdateTool(ctx, 10*time.Second, 4096, executable, "--update-info")
	if err != nil {
		return err
	}
	var actual BuildInfo
	if err = decodeJSON(data, &actual, true); err != nil {
		return fmt.Errorf("更新程序返回的构建身份无效")
	}
	if err = actual.Validate(); err != nil {
		return err
	}
	if actual != expected {
		return fmt.Errorf("更新程序构建身份与发布清单不匹配")
	}
	return ctx.Err()
}
