package session

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
)

// 在真实 TCP 上停止 SSH 对端的读写；Close 仅用于夹具最终回收。
type stoppedPeer struct {
	net.Conn
	paused  atomic.Bool
	entered chan struct{}
	release chan struct{}
	once    sync.Once
	closed  sync.Once
}

func newStoppedPeer(conn net.Conn) *stoppedPeer {
	if tcp, ok := conn.(*net.TCPConn); ok {
		_ = tcp.SetReadBuffer(1024)
	}
	return &stoppedPeer{Conn: conn, entered: make(chan struct{}), release: make(chan struct{})}
}

func (p *stoppedPeer) wait() {
	if p.paused.Load() {
		p.once.Do(func() { close(p.entered) })
		<-p.release
	}
}

func (p *stoppedPeer) Read(data []byte) (int, error) {
	p.wait()
	n, err := p.Conn.Read(data)
	p.wait()
	return n, err
}

func (p *stoppedPeer) Write(data []byte) (int, error) {
	p.wait()
	return p.Conn.Write(data)
}

func (p *stoppedPeer) resume() {
	p.closed.Do(func() { close(p.release) })
}

func (p *stoppedPeer) Close() error {
	p.resume()
	return p.Conn.Close()
}

func waitP2(t *testing.T, done <-chan struct{}, message string) {
	t.Helper()
	select {
	case <-done:
	case <-time.After(3 * time.Second):
		t.Fatal(message)
	}
}

func TestSFTPFullyStoppedPeerCancellation(t *testing.T) {
	for _, phase := range []string{"open_channel", "subsystem", "version"} {
		t.Run(phase, func(t *testing.T) {
			transport := make(chan *stoppedPeer, 1)
			entered := make(chan struct{})
			var control atomic.Pointer[stoppedPeer]
			var sessions atomic.Int32
			server := startLoopbackConfigured(t, loopbackOptions{
				transport: func(conn net.Conn) net.Conn {
					p := newStoppedPeer(conn)
					control.Store(p)
					transport <- p
					return p
				},
				channel: func(_ *ssh.ServerConn, channel ssh.NewChannel) bool {
					if channel.ChannelType() == "session" && sessions.Add(1) == 2 && phase == "open_channel" {
						control.Load().paused.Store(true)
						close(entered)
						return true
					}
					return false
				},
			}, func(channel ssh.Channel, request *ssh.Request) bool {
				if phase == "version" {
					_ = request.Reply(true, nil)
					var init [9]byte
					if _, err := io.ReadFull(channel, init[:]); err != nil {
						t.Error("未收到 SFTP INIT", err)
					}
				}
				control.Load().paused.Store(true)
				close(entered)
				return true
			}, nil)
			manager, _ := newLoopbackManager(t, server, "tester")
			opened, err := manager.Connect("loopback")
			if err != nil {
				t.Fatal(err)
			}
			peer := <-transport
			t.Cleanup(func() { _ = peer.Close() })
			snapshot := waitConnected(t, manager, opened, true)
			session, _ := manager.Get(snapshot.ID, snapshot.Generation)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			done := make(chan error, 1)
			go func() {
				_, err := manager.ReadDirectory(ctx, snapshot.ID, snapshot.Generation, remoteTestPath(server.root))
				done <- err
			}()
			waitP2(t, entered, "未进入完全停滞的 SFTP 阶段")
			cancel()
			select {
			case err = <-done:
				if !errors.Is(err, context.Canceled) {
					t.Fatal(err)
				}
			case <-time.After(3 * time.Second):
				_ = peer.Close()
				<-done
				t.Fatal("完全停滞远端阻塞 SFTP 取消")
			}
			waitP2(t, session.done, "取消后宿主会话 worker 未退出")
			ctx, stop := context.WithTimeout(context.Background(), time.Second)
			defer stop()
			release, err := manager.Drain(ctx)
			if err != nil {
				t.Fatal(err)
			}
			release()
		})
	}
}

func TestSSHTransportWriteBackpressureCancellation(t *testing.T) {
	for _, jump := range []bool{false, true} {
		t.Run(fmt.Sprintf("proxy_jump_%t", jump), func(t *testing.T) {
			transport := make(chan *stoppedPeer, 1)
			options := loopbackOptions{transport: func(conn net.Conn) net.Conn {
				peer := newStoppedPeer(conn)
				transport <- peer
				return peer
			}}
			var server loopback
			var jumpServer loopback
			if jump {
				server = startLoopback(t)
				jumpServer = startLoopbackConfigured(t, options, nil, nil)
			} else {
				server = startLoopbackConfigured(t, options, nil, nil)
			}
			manager, repo := newLoopbackManager(t, server, "tester")
			if jump {
				profile, _ := repo.Profile("loopback")
				profile.ProxyJump = "tester@" + jumpServer.address
				if _, err := repo.SaveProfile(context.Background(), model.SaveProfileRequest{ExpectedRevision: repo.Snapshot().Revision, OriginalName: profile.Name, Profile: profile}); err != nil {
					t.Fatal(err)
				}
			}
			opened, err := manager.Connect("loopback")
			if err != nil {
				t.Fatal(err)
			}
			snapshot := waitConnected(t, manager, opened, true)
			peer := <-transport
			t.Cleanup(func() { _ = peer.Close() })
			session, _ := manager.Get(snapshot.ID, snapshot.Generation)
			session.mu.Lock()
			host := session.client
			session.mu.Unlock()
			peer.paused.Store(true)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			var sent atomic.Int32
			done := make(chan error, 1)
			go func() {
				_, err := negotiation(ctx, host, nil, func() (bool, error) {
					payload := make([]byte, 64<<10)
					for range 4096 {
						if _, _, err := host.SendRequest("kitonyterms-stall-test", false, payload); err != nil {
							return false, err
						}
						sent.Add(1)
					}
					return true, fmt.Errorf("测试未形成 TCP 写背压")
				})
				done <- err
			}()
			waitP2(t, peer.entered, "对端未停止传输")
			// 连续发送无回复请求，不受 SSH 通道窗口限制；进度停止说明实际写入阻塞。
			deadline := time.Now().Add(3 * time.Second)
			blocked := false
			for time.Now().Before(deadline) {
				before := sent.Load()
				time.Sleep(100 * time.Millisecond)
				if before > 0 && sent.Load() == before {
					blocked = true
					break
				}
			}
			if !blocked {
				_ = peer.Close()
				<-done
				t.Fatal("未观测到实际 TCP 写背压")
			}
			t.Logf("停读后 %d 个 64KiB 请求形成传输写背压", sent.Load())
			cancel()
			select {
			case err = <-done:
				if !errors.Is(err, context.Canceled) {
					t.Fatal(err)
				}
			case <-time.After(2 * time.Second):
				_ = peer.Close()
				<-done
				t.Fatal("传输写背压阻塞实际取消/关闭")
			}
			waitP2(t, session.done, "写背压取消后会话 worker 未退出")
			ctx, stop := context.WithTimeout(context.Background(), time.Second)
			defer stop()
			release, err := manager.Drain(ctx)
			if err != nil {
				t.Fatal(err)
			}
			release()
		})
	}
}
