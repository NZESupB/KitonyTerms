//go:build windows

package session

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"io"
	"net"
	"sync"
	"testing"
	"time"

	"github.com/Microsoft/go-winio"
	"golang.org/x/crypto/ssh"
	"golang.org/x/crypto/ssh/agent"
	"golang.org/x/sys/windows"
)

func dialTestAgent(ctx context.Context, path string) (net.Conn, error) {
	return dialAgentPipe(ctx, path)
}

func listenTestAgent(t *testing.T) (string, net.Listener) {
	t.Helper()
	user, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		t.Fatal(err)
	}
	path := `\\.\pipe\KitonyTerms-Agent-Acceptance-` + rand.Text()
	listener, err := winio.ListenPipe(path, &winio.PipeConfig{
		SecurityDescriptor: "D:P(A;;GA;;;" + user.User.Sid.String() + ")",
		InputBufferSize:    1024,
		OutputBufferSize:   1024,
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	return path, listener
}

// 私有原生管道提供agent协议；不访问Windows OpenSSH系统服务或用户密钥。
func isolatedNativeAgent(t *testing.T) (string, ssh.PublicKey) {
	t.Helper()
	_, private, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	keyring := agent.NewKeyring()
	if err = keyring.Add(agent.AddedKey{PrivateKey: private}); err != nil {
		t.Fatal(err)
	}
	public, err := ssh.NewPublicKey(private.Public())
	if err != nil {
		t.Fatal(err)
	}
	path, listener := listenTestAgent(t)
	var connections sync.Map
	var workers sync.WaitGroup
	accepted := make(chan struct{})
	go func() {
		defer close(accepted)
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			connections.Store(conn, true)
			workers.Add(1)
			go func() {
				defer workers.Done()
				defer connections.Delete(conn)
				defer conn.Close()
				_ = agent.ServeAgent(keyring, conn)
			}()
		}
	}()
	t.Cleanup(func() {
		_ = listener.Close()
		waitP2(t, accepted, "私有agent管道监听未结束")
		connections.Range(func(key, _ any) bool { _ = key.(net.Conn).Close(); return true })
		done := make(chan struct{})
		go func() { workers.Wait(); close(done) }()
		waitP2(t, done, "私有agent协议服务未实际结束")
		_ = keyring.RemoveAll()
		clear(private)
		ctx, cancel := context.WithTimeout(context.Background(), time.Second)
		defer cancel()
		if conn, err := dialTestAgent(ctx, path); err == nil {
			_ = conn.Close()
			t.Error("私有agent管道未释放")
		}
	})
	return path, public
}

func TestWindowsAgentPipeDialCancellationAndErrors(t *testing.T) {
	t.Run("pre_canceled_default", func(t *testing.T) {
		ctx, cancel := context.WithCancel(context.Background())
		cancel()
		// 预先取消保证默认入口不会接触系统agent端点。
		if conn, err := openAgent(ctx); conn != nil || !errors.Is(err, context.Canceled) {
			t.Fatal("默认入口忽略预先取消", err)
		}
	})
	t.Run("missing_private_endpoint", func(t *testing.T) {
		path := `\\.\pipe\KitonyTerms-Agent-Missing-` + rand.Text()
		if conn, err := dialAgentPipe(context.Background(), path); conn != nil || !errors.Is(err, windows.ERROR_FILE_NOT_FOUND) {
			t.Fatal("缺失私有端点的Win32原因丢失", err)
		}
	})
	t.Run("busy_endpoint", func(t *testing.T) {
		path, _ := listenTestAgent(t)
		ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
		defer cancel()
		started := time.Now()
		if conn, err := dialAgentPipe(ctx, path); conn != nil || !errors.Is(err, context.DeadlineExceeded) || time.Since(started) > time.Second {
			t.Fatal("忙碌私有管道未按context收敛", err)
		}
	})
}

func TestWindowsAgentPipePendingIOAndClose(t *testing.T) {
	for _, operation := range []string{"read", "write", "deadline"} {
		t.Run(operation, func(t *testing.T) {
			path, listener := listenTestAgent(t)
			accepted := make(chan net.Conn, 1)
			acceptErr := make(chan error, 1)
			go func() {
				conn, err := listener.Accept()
				if err != nil {
					acceptErr <- err
					return
				}
				accepted <- conn
			}()
			client, err := dialAgentPipe(context.Background(), path)
			if err != nil {
				t.Fatal(err)
			}
			defer client.Close()
			var server net.Conn
			select {
			case server = <-accepted:
			case err = <-acceptErr:
				t.Fatal(err)
			case <-time.After(time.Second):
				t.Fatal("私有管道未接受连接")
			}
			defer server.Close()
			if operation == "deadline" {
				if err = client.SetReadDeadline(time.Now().Add(50 * time.Millisecond)); err != nil {
					t.Fatal(err)
				}
			}
			done := make(chan error, 1)
			go func() {
				if operation == "write" {
					_, err := client.Write(make([]byte, 8<<20))
					done <- err
				} else {
					_, err := io.ReadFull(client, make([]byte, 5))
					done <- err
				}
			}()
			if operation != "deadline" {
				select {
				case err = <-done:
					t.Fatal("夹具未形成挂起I/O", err)
				case <-time.After(20 * time.Millisecond):
				}
				if err = client.Close(); err != nil {
					t.Fatal(err)
				}
			}
			select {
			case err = <-done:
				if err == nil {
					t.Fatal("挂起管道I/O被误报为成功")
				}
				if operation == "deadline" {
					var failure net.Error
					if !errors.As(err, &failure) || !failure.Timeout() {
						t.Fatal("管道期限错误分类丢失", err)
					}
				}
			case <-time.After(time.Second):
				t.Fatal("Close或期限后I/O未实际结束")
			}
		})
	}
}
