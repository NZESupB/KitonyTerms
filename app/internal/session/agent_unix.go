//go:build !windows

package session

import (
	"context"
	"fmt"
	"io"
	"net"
	"os"
	"time"
)

func openAgent(ctx context.Context) (io.ReadWriteCloser, error) {
	socket := os.Getenv("SSH_AUTH_SOCK")
	if socket == "" {
		return nil, fmt.Errorf("当前平台没有可访问的 SSH agent")
	}
	return (&net.Dialer{Timeout: 10 * time.Second}).DialContext(ctx, "unix", socket)
}
