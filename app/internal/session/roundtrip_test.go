package session

import (
	"bytes"
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/pkg/sftp"
	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
	"kitonyterms/internal/storage"
)

type loopback struct {
	address     string
	fingerprint string
	root        string
}

// Windows SFTP服务端使用带斜杠前缀的盘符路径；本机文件操作仍使用原路径。
func remoteTestPath(local string) string {
	value := filepath.ToSlash(local)
	if filepath.VolumeName(local) != "" {
		return "/" + value
	}
	return value
}

func startLoopback(t *testing.T) loopback {
	return startLoopbackSFTP(t, nil)
}

func startLoopbackSFTP(t *testing.T, intercept func(ssh.Channel, *ssh.Request) bool) loopback {
	return startLoopbackHooks(t, intercept, nil)
}

type loopbackOptions struct {
	configure func(*ssh.ServerConfig)
	transport func(net.Conn) net.Conn
	connected func(*ssh.ServerConn)
	channel   func(*ssh.ServerConn, ssh.NewChannel) bool
}

func startLoopbackHooks(t *testing.T, intercept func(ssh.Channel, *ssh.Request) bool, execIntercept func(ssh.Channel, *ssh.Request) bool, requestHooks ...func(ssh.Channel, *ssh.Request) bool) loopback {
	return startLoopbackConfigured(t, loopbackOptions{}, intercept, execIntercept, requestHooks...)
}

func startLoopbackConfigured(t *testing.T, options loopbackOptions, intercept func(ssh.Channel, *ssh.Request) bool, execIntercept func(ssh.Channel, *ssh.Request) bool, requestHooks ...func(ssh.Channel, *ssh.Request) bool) loopback {

	t.Helper()
	_, key, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	signer, err := ssh.NewSignerFromKey(key)
	if err != nil {
		t.Fatal(err)
	}
	config := &ssh.ServerConfig{PasswordCallback: func(_ ssh.ConnMetadata, password []byte) (*ssh.Permissions, error) {
		if string(password) != "test-password" {
			return nil, fmt.Errorf("authentication denied")
		}
		return nil, nil
	}}
	if options.configure != nil {
		options.configure(config)
	}
	config.AddHostKey(signer)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	root := t.TempDir()
	var wg sync.WaitGroup
	var connections sync.Map
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			if options.transport != nil {
				conn = options.transport(conn)
			}
			connections.Store(conn, true)
			wg.Add(1)
			go func() {
				defer wg.Done()
				defer connections.Delete(conn)
				defer conn.Close()
				server, channels, requests, err := ssh.NewServerConn(conn, config)
				if err != nil {
					return
				}
				defer server.Close()
				if options.connected != nil {
					options.connected(server)
				}
				go ssh.DiscardRequests(requests)
				var tasks sync.WaitGroup
				for request := range channels {
					if options.channel != nil && options.channel(server, request) {
						continue
					}
					if request.ChannelType() == "direct-tcpip" {
						var target struct {
							Host       string
							Port       uint32
							Origin     string
							OriginPort uint32
						}
						if ssh.Unmarshal(request.ExtraData(), &target) != nil || target.Host != "127.0.0.1" {
							request.Reject(ssh.Prohibited, "loopback only")
							continue
						}
						upstream, err := net.DialTimeout("tcp", net.JoinHostPort(target.Host, strconv.Itoa(int(target.Port))), time.Second)
						if err != nil {
							request.Reject(ssh.ConnectionFailed, "dial failed")
							continue
						}
						channel, requests, err := request.Accept()
						if err != nil {
							upstream.Close()
							continue
						}
						go ssh.DiscardRequests(requests)
						tasks.Add(1)
						go func() {
							defer tasks.Done()
							defer upstream.Close()
							defer channel.Close()
							go func() { io.Copy(upstream, channel); upstream.Close() }()
							io.Copy(channel, upstream)
						}()
						continue
					}
					if request.ChannelType() != "session" {
						request.Reject(ssh.UnknownChannelType, "unsupported")
						continue
					}
					channel, incoming, err := request.Accept()
					if err != nil {
						continue
					}
					tasks.Add(1)
					go func() {
						defer tasks.Done()
						defer channel.Close()
						for req := range incoming {
							if len(requestHooks) > 0 && requestHooks[0](channel, req) {
								continue
							}
							switch req.Type {
							case "pty-req", "window-change":
								req.Reply(true, nil)
							case "shell":
								req.Reply(true, nil)
								if server.User() == "flood" {
									_, _ = channel.Write(bytes.Repeat([]byte("f"), 2<<20))
								} else {
									_, _ = channel.Write([]byte("ready\r\n"))
								}
								_, _ = io.Copy(channel, channel)
								return
							case "subsystem":
								var subsystem struct{ Name string }
								if ssh.Unmarshal(req.Payload, &subsystem) != nil || subsystem.Name != "sftp" {
									req.Reply(false, nil)
									continue
								}
								if intercept != nil && intercept(channel, req) {
									return
								}
								req.Reply(true, nil)
								fs, err := sftp.NewServer(channel, sftp.WithServerWorkingDirectory(root))
								if err == nil {
									_ = fs.Serve()
									_ = fs.Close()
								}
								return
							case "exec":
								if execIntercept != nil && execIntercept(channel, req) {
									return
								}
								req.Reply(true, nil)
								_, _ = channel.Write([]byte("operation-ok\n"))
								_, _ = channel.SendRequest("exit-status", false, ssh.Marshal(struct{ Status uint32 }{0}))
								return
							default:
								req.Reply(false, nil)
							}
						}
					}()
				}
				tasks.Wait()
			}()
		}
	}()
	t.Cleanup(func() {
		listener.Close()
		connections.Range(func(key, value any) bool { key.(net.Conn).Close(); return true })
		wg.Wait()
	})
	return loopback{listener.Addr().String(), ssh.FingerprintSHA256(signer.PublicKey()), root}
}
func newLoopbackManager(t *testing.T, server loopback, user string) (*Manager, *storage.Repository) {
	t.Helper()
	dir := t.TempDir()
	repo, err := storage.OpenRepository(dir, dir)
	if err != nil {
		t.Fatal(err)
	}
	settings := repo.Snapshot()
	settings.Config.Settings.UseSSHConfig = false
	if _, err = repo.SaveSettings(context.Background(), model.SaveSettingsRequest{ExpectedRevision: settings.Revision, Settings: settings.Config.Settings}); err != nil {
		t.Fatal(err)
	}
	host, portText, err := net.SplitHostPort(server.address)
	if err != nil {
		t.Fatal(err)
	}
	port, _ := strconv.Atoi(portText)
	_, err = repo.SaveProfile(context.Background(), model.SaveProfileRequest{ExpectedRevision: repo.Snapshot().Revision, Profile: model.Profile{Name: "loopback", Host: host, Port: port, User: user, Auth: []model.AuthMethod{{Kind: "password"}}, Proxy: model.Proxy{Kind: "direct"}}})
	if err != nil {
		t.Fatal(err)
	}
	vault, err := storage.OpenVault(dir)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(vault.Close)
	manager := New(context.Background(), repo, vault, func() {})
	t.Cleanup(manager.Shutdown)
	return manager, repo
}
func waitConnected(t *testing.T, m *Manager, session model.SessionSnapshot, trust bool) model.SessionSnapshot {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		s, err := m.Get(session.ID, session.Generation)
		if err != nil {
			t.Fatal(err)
		}
		snapshot := s.snapshot()
		if snapshot.Status == "connected" {
			return snapshot
		}
		if snapshot.Status == "closed" {
			t.Fatalf("SSH 连接失败: %s", snapshot.Error)
		}
		if c := snapshot.Challenge; c != nil {
			a := model.ChallengeAnswer{SessionID: session.ID, Generation: session.Generation, ChallengeID: c.ID, Trust: trust}
			if c.Kind == "password" {
				a.Answers = []string{"test-password"}
			}
			if err = m.Answer(a); err != nil {
				t.Fatal(err)
			}
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("SSH 连接超时")
	return model.SessionSnapshot{}
}
func TestSSHAuthenticationTerminalAndSFTPRoundtrip(t *testing.T) {
	server := startLoopback(t)
	m, repo := newLoopbackManager(t, server, "tester")
	opened, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	session := waitConnected(t, m, opened, true)
	host, portText, _ := net.SplitHostPort(server.address)
	port, _ := strconv.Atoi(portText)
	if _, ok := repo.CheckHost(host, port, server.fingerprint); !ok {
		t.Fatal("主机信任未写入")
	}
	if err = m.Resize(session.ID, session.Generation, 120, 40); err != nil {
		t.Fatal(err)
	}
	input := model.InputRequest{SessionID: session.ID, Generation: session.Generation, Sequence: "1", Data: base64.StdEncoding.EncodeToString([]byte("echo-中文\n"))}
	if err = m.Input(input); err != nil {
		t.Fatal(err)
	}
	if err = m.Input(input); err != nil {
		t.Fatal("重试未去重", err)
	}
	input.Data = base64.StdEncoding.EncodeToString([]byte("different"))
	if err = m.Input(input); err == nil {
		t.Fatal("同序不同数据被接受")
	}
	state, err := m.TerminalState(session.ID, session.Generation)
	if err != nil || state.InputSequence != "1" {
		t.Fatal("输入序号恢复错误", err)
	}
	var output strings.Builder
	after := "0"
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		batch, err := m.Poll(session.ID, session.Generation, after)
		if err != nil {
			t.Fatal(err)
		}
		for _, chunk := range batch.Chunks {
			data, _ := base64.StdEncoding.DecodeString(chunk.Data)
			output.Write(data)
			after = chunk.Sequence
		}
		if strings.Contains(output.String(), "echo-中文") {
			break
		}
		time.Sleep(5 * time.Millisecond)
	}
	if strings.Count(output.String(), "echo-中文") != 1 {
		t.Fatalf("输入输出不匹配: %q", output.String())
	}
	remote := filepath.ToSlash(filepath.Join(server.root, "文本.txt"))
	if err = os.WriteFile(remote, []byte("before\n"), 0640); err != nil {
		t.Fatal(err)
	}
	initialInfo, err := os.Stat(remote)
	if err != nil {
		t.Fatal(err)
	}
	remotePath := remoteTestPath(remote)
	directory, err := m.ReadDirectory(context.Background(), session.ID, session.Generation, remoteTestPath(server.root))
	if err != nil || len(directory.Entries) != 1 {
		t.Fatal("SFTP 目录读取失败", err)
	}
	text, err := m.ReadText(context.Background(), session.ID, session.Generation, remotePath)
	if err != nil {
		t.Fatal(err)
	}
	updated, err := m.WriteText(context.Background(), session.ID, session.Generation, remotePath, "after 中文\n", text.Revision)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = m.WriteText(context.Background(), session.ID, session.Generation, remotePath, "bad", text.Revision); err == nil {
		t.Fatal("过期编辑覆盖远端")
	}
	info, err := os.Stat(remote)
	if err != nil || info.Mode().Perm() != initialInfo.Mode().Perm() {
		t.Fatal("编辑丢失权限", err)
	}
	local := filepath.Join(t.TempDir(), "download.txt")
	if err = m.Download(context.Background(), session.ID, session.Generation, remotePath, local); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(local)
	if err != nil || string(data) != updated.Content {
		t.Fatal("下载内容错误", err)
	}
	if err = m.Upload(context.Background(), session.ID, session.Generation, remoteTestPath(server.root), local); err != nil {
		t.Fatal(err)
	}
	if err = m.Upload(context.Background(), session.ID, session.Generation, remoteTestPath(server.root), local); err == nil {
		t.Fatal("上传静默覆盖")
	}
	operation, err := m.RunOperation(context.Background(), session.ID, session.Generation, "status", "")
	if err != nil || operation.Output != "operation-ok\n" {
		t.Fatal("exec 通道失败", err)
	}
	if err = m.CloseSession(session.ID, session.Generation); err != nil {
		t.Fatal(err)
	}
	if len(m.Snapshots()) != 0 {
		t.Fatal("关闭会话没有回收")
	}
}
func TestSSHChangedHostKeyAndShutdownUnderBackpressure(t *testing.T) {
	server := startLoopback(t)
	m, repo := newLoopbackManager(t, server, "flood")
	snapshot, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	waitConnected(t, m, snapshot, true)
	time.Sleep(30 * time.Millisecond)
	done := make(chan struct{})
	go func() { m.Shutdown(); close(done) }()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		t.Fatal("输出背压阻塞关闭")
	}
	if _, err = m.Connect("loopback"); err == nil {
		t.Fatal("关闭后仍接受连接")
	}
	host, portText, _ := net.SplitHostPort(server.address)
	port, _ := strconv.Atoi(portText)
	if err = repo.TrustHost(host, port, "SHA256:changed"); err == nil {
		t.Fatal("变更主机密钥被接受")
	}
}

func TestProxyJumpNegotiatesNestedSSH(t *testing.T) {
	target := startLoopback(t)
	jump := startLoopback(t)
	m, repo := newLoopbackManager(t, target, "tester")
	p, err := repo.Profile("loopback")
	if err != nil {
		t.Fatal(err)
	}
	p.ProxyJump = "tester@" + jump.address
	if _, err = repo.SaveProfile(context.Background(), model.SaveProfileRequest{ExpectedRevision: repo.Snapshot().Revision, OriginalName: p.Name, Profile: p}); err != nil {
		t.Fatal(err)
	}
	opened, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	connected := waitConnected(t, m, opened, true)
	if connected.Status != "connected" {
		t.Fatal("跳板连接未完成")
	}
}
