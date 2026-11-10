//go:build windows

package session

import (
	"context"
	"io"
	"net"
	"time"

	"github.com/Microsoft/go-winio"
)

// 保持Windows OpenSSH默认端点；连接及忙碌等待由成熟管道库处理。
func openAgent(ctx context.Context) (io.ReadWriteCloser, error) {
	return dialAgentPipe(ctx, `\\.\pipe\openssh-ssh-agent`)
}

func dialAgentPipe(ctx context.Context, path string) (net.Conn, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	return winio.DialPipeContext(ctx, path)
}
