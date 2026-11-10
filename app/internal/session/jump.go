package session

import (
	"context"
	"errors"
	"net"

	"golang.org/x/crypto/ssh"
)

// 单跳连接由当前会话独占；先切断宿主传输，避免通道 Close 卡在跳板写背压上。
type jumpConn struct {
	net.Conn
	host *ssh.Client
}

func (c *jumpConn) Close() error {
	return errors.Join(c.host.Close(), c.Conn.Close())
}

func dialJump(ctx context.Context, host *ssh.Client, address string) (net.Conn, error) {
	conn, err := negotiation(ctx, host, nil, func() (net.Conn, error) {
		return host.Dial("tcp", address)
	})
	if err != nil {
		_ = host.Close()
		if conn != nil {
			_ = conn.Close()
		}
		return nil, err
	}
	return &jumpConn{Conn: conn, host: host}, nil
}
