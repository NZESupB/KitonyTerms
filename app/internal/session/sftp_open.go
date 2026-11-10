package session

import (
	"context"
	"time"

	"github.com/pkg/sftp"
	"golang.org/x/crypto/ssh"
)

// SSH 不提供取消 OpenChannel 的协议；对拒绝响应关闭的远端，限时回收整条连接。
func negotiation[T any](ctx context.Context, host *ssh.Client, channel ssh.Channel, run func() (T, error)) (T, error) {
	type result struct {
		value T
		err   error
	}
	if err := ctx.Err(); err != nil {
		var zero T
		return zero, err
	}
	done := make(chan result, 1)
	go func() { value, err := run(); done <- result{value, err} }()
	select {
	case value := <-done:
		return value.value, value.err
	case <-ctx.Done():
		closed := make(chan struct{})
		go func() {
			if channel != nil {
				_ = channel.Close()
			}
			close(closed)
		}()
		timer := time.NewTimer(500 * time.Millisecond)
		defer timer.Stop()
		var value result
		resultChannel, closeChannel := done, closed
		for resultChannel != nil || closeChannel != nil {
			select {
			case value = <-resultChannel:
				resultChannel = nil
			case <-closeChannel:
				closeChannel = nil
			case <-timer.C:
				_ = host.Close()
			}
		}
		return value.value, ctx.Err()
	}
}

func openSFTP(ctx context.Context, host *ssh.Client) (*sftp.Client, error) {
	ctx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	type opened struct {
		channel  ssh.Channel
		requests <-chan *ssh.Request
	}
	openedChannel, err := negotiation(ctx, host, nil, func() (opened, error) {
		channel, requests, err := host.OpenChannel("session", nil)
		return opened{channel, requests}, err
	})
	channel := openedChannel.channel
	if err != nil {
		if channel != nil {
			_ = channel.Close()
		}
		return nil, err
	}
	keep := false
	defer func() {
		if !keep {
			_ = channel.Close()
		}
	}()
	go ssh.DiscardRequests(openedChannel.requests)
	_, err = negotiation(ctx, host, channel, func() (bool, error) {
		ok, err := channel.SendRequest("subsystem", true, ssh.Marshal(struct{ Name string }{"sftp"}))
		if err == nil && !ok {
			err = &ssh.OpenChannelError{Reason: ssh.Prohibited, Message: "远端拒绝 SFTP 子系统"}
		}
		return ok, err
	})
	if err != nil {
		return nil, err
	}
	transport := newSFTPTransport(host, channel)
	defer func() {
		if !keep {
			_ = transport.Close()
		}
	}()
	client, err := negotiation(ctx, host, channel, func() (*sftp.Client, error) {
		return sftp.NewClientPipe(transport, transport, sftp.MaxPacket(32768))
	})
	if err != nil {
		if client != nil {
			_ = client.Close()
		}
		return nil, err
	}
	keep = true
	return client, nil
}
