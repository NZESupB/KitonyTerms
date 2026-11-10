//go:build !windows && !android && !ios

package session

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
	"golang.org/x/crypto/ssh/agent"
)

func dialTestAgent(ctx context.Context, socket string) (net.Conn, error) {
	return (&net.Dialer{Timeout: 10 * time.Second}).DialContext(ctx, "unix", socket)
}

func listenTestAgent(t *testing.T) (string, net.Listener) {
	t.Helper()
	dir, err := os.MkdirTemp("", "kt-agent-stall-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	socket := filepath.Join(dir, "a.sock")
	listener, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = listener.Close() })
	return socket, listener
}

func isolatedNativeAgent(t *testing.T) (string, ssh.PublicKey) {
	t.Helper()
	binary, err := exec.LookPath("ssh-agent")
	if err != nil {
		t.Skip("本机未提供系统 ssh-agent")
	}
	// Unix socket 路径有平台长度限制；私有短目录与用户 agent 完全隔离。
	dir, err := os.MkdirTemp("", "kt-agent-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(dir) })
	socket := filepath.Join(dir, "a.sock")
	cmd := exec.Command(binary, "-D", "-a", socket)
	cmd.Env = []string{"PATH=/usr/bin:/bin", "SSH_AUTH_SOCK=" + socket}
	cmd.Stdout, cmd.Stderr = io.Discard, io.Discard
	if err = cmd.Start(); err != nil {
		t.Fatal(err)
	}
	exited := make(chan error, 1)
	go func() { exited <- cmd.Wait() }()
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		select {
		case <-exited:
		case <-time.After(3 * time.Second):
			t.Error("临时系统 agent 未实际退出")
		}
	})
	var conn net.Conn
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		conn, err = net.DialTimeout("unix", socket, 100*time.Millisecond)
		if err == nil {
			break
		}
		time.Sleep(10 * time.Millisecond)
	}
	if err != nil {
		t.Fatal("临时系统 agent 未就绪", err)
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(3 * time.Second))
	_, private, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	if err = agent.NewClient(conn).Add(agent.AddedKey{PrivateKey: private, Comment: "KitonyTerms 隔离回归"}); err != nil {
		t.Fatal("向临时 agent 添加生成密钥失败", err)
	}
	public, err := ssh.NewPublicKey(private.Public())
	if err != nil {
		t.Fatal(err)
	}
	return socket, public
}
