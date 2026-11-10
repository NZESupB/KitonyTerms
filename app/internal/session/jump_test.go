package session

import (
	"context"
	"fmt"
	"io"
	"net"
	"strconv"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
)

type silentHandshakePeer struct {
	*stoppedPeer
	once    sync.Once
	entered chan struct{}
	err     error
}

func (p *silentHandshakePeer) Write(data []byte) (int, error) {
	p.once.Do(func() {
		var banner [4]byte
		_, p.err = io.ReadFull(p.Conn, banner[:])
		if p.err == nil && string(banner[:]) != "SSH-" {
			p.err = fmt.Errorf("内层握手版本前缀无效")
		}
		if p.err == nil {
			p.paused.Store(true)
			close(p.entered)
		}
	})
	if p.err != nil {
		return 0, p.err
	}
	return p.stoppedPeer.Write(data)
}

func TestProxyJumpDrainWhileOpening(t *testing.T) {
	for _, phase := range []string{"forward_open", "inner_handshake"} {
		t.Run(phase, func(t *testing.T) {
			entered := make(chan struct{})
			var jumpControl atomic.Pointer[stoppedPeer]
			jumpConnections := make(chan *ssh.ServerConn, 1)
			jump := startLoopbackConfigured(t, loopbackOptions{
				transport: func(conn net.Conn) net.Conn {
					peer := newStoppedPeer(conn)
					jumpControl.Store(peer)
					return peer
				},
				connected: func(conn *ssh.ServerConn) { jumpConnections <- conn },
				channel: func(_ *ssh.ServerConn, channel ssh.NewChannel) bool {
					if phase == "forward_open" && channel.ChannelType() == "direct-tcpip" {
						jumpControl.Load().paused.Store(true)
						close(entered)
						return true
					}
					return false
				},
			}, nil, nil)
			targetOptions := loopbackOptions{}
			if phase == "inner_handshake" {
				targetOptions.transport = func(conn net.Conn) net.Conn {
					return &silentHandshakePeer{stoppedPeer: newStoppedPeer(conn), entered: entered}
				}
			}
			target := startLoopbackConfigured(t, targetOptions, nil, nil)
			manager, repo := newLoopbackManager(t, target, "tester")
			profile, err := repo.Profile("loopback")
			if err != nil {
				t.Fatal(err)
			}
			profile.ProxyJump = "tester@" + jump.address
			if err = manager.vault.Set(profile.SecretID(), "test-password"); err != nil {
				t.Fatal(err)
			}
			jumpHost, jumpPort, _ := net.SplitHostPort(jump.address)
			port, _ := strconv.Atoi(jumpPort)
			if err = manager.vault.Set((model.Profile{Host: jumpHost, Port: port, User: profile.User}).SecretID(), "test-password"); err != nil {
				t.Fatal(err)
			}
			if err = repo.TrustHost(jumpHost, port, jump.fingerprint); err != nil {
				t.Fatal(err)
			}
			if _, err = repo.SaveProfile(context.Background(), model.SaveProfileRequest{ExpectedRevision: repo.Snapshot().Revision, OriginalName: profile.Name, Profile: profile}); err != nil {
				t.Fatal(err)
			}
			opened, err := manager.Connect(profile.Name)
			if err != nil {
				t.Fatal(err)
			}
			session, _ := manager.Get(opened.ID, opened.Generation)
			waitP2(t, entered, "未进入跳板停滞阶段")
			peer := jumpControl.Load()
			t.Cleanup(func() { _ = peer.Close() })
			peer.paused.Store(true)
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			release, err := manager.Drain(ctx)
			if err != nil {
				_ = peer.Close()
				t.Fatal("跳板准入或内层握手未实际回收", err)
			}
			defer release()
			waitP2(t, session.done, "跳板取消后连接 worker 未退出")
			conn := <-jumpConnections
			done := make(chan struct{})
			go func() { _ = conn.Wait(); close(done) }()
			// 只恢复读取，不主动关闭对端，确认本地回收已使传输结束。
			peer.resume()
			waitP2(t, done, "跳板传输未实际结束")
		})
	}
}
