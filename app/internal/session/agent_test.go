//go:build !android && !ios

package session

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
	"golang.org/x/crypto/ssh/agent"
	"kitonyterms/internal/model"
)

func TestNativeAgentAuthenticationAndForwarding(t *testing.T) {
	socket, public := isolatedNativeAgent(t)
	for _, scenario := range []struct {
		name    string
		forward bool
		jump    bool
	}{{"authentication_only", false, false}, {"forwarding", true, false}, {"jump_forwarding_target_only", true, true}} {
		t.Run(scenario.name, func(t *testing.T) {
			var authenticated, forwarding atomic.Int32
			servers := make(chan *ssh.ServerConn, 2)
			options := loopbackOptions{
				configure: func(config *ssh.ServerConfig) {
					config.PasswordCallback = nil
					config.PublicKeyCallback = func(_ ssh.ConnMetadata, key ssh.PublicKey) (*ssh.Permissions, error) {
						if !bytes.Equal(key.Marshal(), public.Marshal()) {
							return nil, fmt.Errorf("非测试 agent 公钥")
						}
						authenticated.Add(1)
						return nil, nil
					}
				},
				connected: func(server *ssh.ServerConn) { servers <- server },
			}
			server := startLoopbackConfigured(t, options, nil, nil, func(_ ssh.Channel, req *ssh.Request) bool {
				if req.Type != "auth-agent-req@openssh.com" {
					return false
				}
				forwarding.Add(1)
				_ = req.Reply(true, nil)
				return true
			})
			manager, repo := newLoopbackManager(t, server, "tester")
			manager.agentDial = func(ctx context.Context) (io.ReadWriteCloser, error) { return dialTestAgent(ctx, socket) }
			profile, err := repo.Profile("loopback")
			if err != nil {
				t.Fatal(err)
			}
			profile.Auth = []model.AuthMethod{{Kind: "agent"}}
			profile.ForwardAgent = scenario.forward
			var jumpServer *ssh.ServerConn
			if scenario.jump {
				jump := startLoopbackConfigured(t, options, nil, nil)
				profile.ProxyJump = "tester@" + jump.address
			}
			if _, err = repo.SaveProfile(context.Background(), model.SaveProfileRequest{ExpectedRevision: repo.Snapshot().Revision, OriginalName: profile.Name, Profile: profile}); err != nil {
				t.Fatal(err)
			}
			opened, err := manager.Connect(profile.Name)
			if err != nil {
				t.Fatal(err)
			}
			snapshot := waitConnected(t, manager, opened, true)
			var target *ssh.ServerConn
			if scenario.jump {
				jumpServer = <-servers
			}
			target = <-servers
			probe := func(server *ssh.ServerConn, allowed bool) {
				t.Helper()
				result := make(chan error, 1)
				go func() {
					channel, requests, err := server.OpenChannel("auth-agent@openssh.com", nil)
					if !allowed {
						if err == nil {
							_ = channel.Close()
							result <- fmt.Errorf("未授权连接接受了 agent 转发通道")
							return
						}
						result <- nil
						return
					}
					if err != nil {
						result <- err
						return
					}
					defer channel.Close()
					go ssh.DiscardRequests(requests)
					client := agent.NewClient(channel)
					keys, err := client.List()
					if err == nil && (len(keys) != 1 || !bytes.Equal(keys[0].Blob, public.Marshal())) {
						err = fmt.Errorf("转发未返回独立 agent 的唯一公钥")
					}
					if err == nil {
						message := []byte("KitonyTerms 原生 agent 转发签名回归")
						var signature *ssh.Signature
						signature, err = client.Sign(public, message)
						if err == nil {
							err = public.Verify(message, signature)
						}
					}
					result <- err
				}()
				select {
				case err := <-result:
					if err != nil {
						t.Fatal(err)
					}
				case <-time.After(3 * time.Second):
					_ = server.Close()
					t.Fatal("agent 转发探测未实际完成")
				}
			}
			probe(target, scenario.forward)
			if jumpServer != nil {
				probe(jumpServer, false)
			}
			wantAuth, wantForward := int32(1), int32(0)
			if scenario.jump {
				wantAuth = 2
			}
			if scenario.forward {
				wantForward = 1
			}
			if authenticated.Load() != wantAuth || forwarding.Load() != wantForward {
				t.Fatalf("认证/转发归属错误: %d/%d", authenticated.Load(), forwarding.Load())
			}
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			defer cancel()
			release, err := manager.Drain(ctx)
			if err != nil {
				t.Fatal(err)
			}
			release()
			for _, server := range []*ssh.ServerConn{target, jumpServer} {
				if server == nil {
					continue
				}
				done := make(chan struct{})
				go func() { _ = server.Wait(); close(done) }()
				select {
				case <-done:
				case <-ctx.Done():
					t.Fatal("Drain 后目标或跳板传输仍存活")
				}
			}
			if _, err = manager.Get(snapshot.ID, snapshot.Generation); err != nil {
				t.Fatal("Drain 释放未恢复准入", err)
			}
		})
	}
	conn, err := dialTestAgent(context.Background(), socket)
	if err != nil {
		t.Fatal("会话回收误关闭了独立系统 agent", err)
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(time.Second))
	if keys, err := agent.NewClient(conn).List(); err != nil || len(keys) != 1 {
		t.Fatal("会话回收污染了 agent 密钥", err)
	}
}

func TestAgentPendingRequestCancellation(t *testing.T) {
	for _, forwarding := range []bool{false, true} {
		t.Run(fmt.Sprintf("forwarding_%t", forwarding), func(t *testing.T) {
			socket, listener := listenTestAgent(t)
			entered, exited := make(chan struct{}), make(chan struct{})
			accepted := make(chan net.Conn, 1)
			go func() {
				defer close(exited)
				conn, err := listener.Accept()
				if err != nil {
					return
				}
				accepted <- conn
				defer conn.Close()
				var data [5]byte
				if _, err = io.ReadFull(conn, data[:]); err != nil {
					return
				}
				close(entered)
				_, _ = io.Copy(io.Discard, conn)
			}()
			servers := make(chan *ssh.ServerConn, 1)
			server := startLoopbackConfigured(t, loopbackOptions{
				configure: func(config *ssh.ServerConfig) {
					if !forwarding {
						config.PasswordCallback = nil
						config.PublicKeyCallback = func(ssh.ConnMetadata, ssh.PublicKey) (*ssh.Permissions, error) { return nil, nil }
					}
				},
				connected: func(server *ssh.ServerConn) { servers <- server },
			}, nil, nil, func(_ ssh.Channel, req *ssh.Request) bool {
				if req.Type == "auth-agent-req@openssh.com" {
					_ = req.Reply(true, nil)
					return true
				}
				return false
			})
			manager, repo := newLoopbackManager(t, server, "tester")
			manager.agentDial = func(ctx context.Context) (io.ReadWriteCloser, error) { return dialTestAgent(ctx, socket) }
			t.Cleanup(func() {
				_ = listener.Close()
				select {
				case conn := <-accepted:
					_ = conn.Close()
				default:
				}
				waitP2(t, exited, "挂起 agent 夹具未退出")
			})
			profile, _ := repo.Profile("loopback")
			var err error
			profile.ForwardAgent = forwarding
			if !forwarding {
				profile.Auth = []model.AuthMethod{{Kind: "agent"}}
			}
			if err = repo.TrustHost(profile.Host, profile.Port, server.fingerprint); err != nil {
				t.Fatal(err)
			}
			if _, err = repo.SaveProfile(context.Background(), model.SaveProfileRequest{ExpectedRevision: repo.Snapshot().Revision, OriginalName: profile.Name, Profile: profile}); err != nil {
				t.Fatal(err)
			}
			opened, err := manager.Connect(profile.Name)
			if err != nil {
				t.Fatal(err)
			}
			forwarded := make(chan error, 1)
			if forwarding {
				waitConnected(t, manager, opened, true)
				server := <-servers
				go func() {
					channel, requests, err := server.OpenChannel("auth-agent@openssh.com", nil)
					if err == nil {
						defer channel.Close()
						go ssh.DiscardRequests(requests)
						_, err = agent.NewClient(channel).List()
					}
					forwarded <- err
				}()
			}
			waitP2(t, entered, "未进入挂起的 agent 身份请求")
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			release, err := manager.Drain(ctx)
			if err != nil {
				t.Fatal(err)
			}
			defer release()
			waitP2(t, exited, "Drain 未关闭挂起的 agent socket")
			if forwarding {
				select {
				case err = <-forwarded:
					if err == nil {
						t.Fatal("未响应的 agent 请求被误报为成功")
					}
				case <-ctx.Done():
					t.Fatal("Drain 后远端转发调用未退出")
				}
			}
		})
	}
}
