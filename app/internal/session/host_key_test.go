package session

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/rsa"
	"errors"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
	"kitonyterms/internal/storage"
)

func hostKeySigners(t *testing.T) (ssh.Signer, ssh.Signer) {
	t.Helper()
	ecdsaKey, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	ecdsaSigner, err := ssh.NewSignerFromKey(ecdsaKey)
	if err != nil {
		t.Fatal(err)
	}
	rsaKey, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	rsaSigner, err := ssh.NewSignerFromKey(rsaKey)
	if err != nil {
		t.Fatal(err)
	}
	return ecdsaSigner, rsaSigner
}

func knownHostHandshake(t *testing.T, server loopback, pin, password string) (*Manager, *Session, model.Profile, string) {
	t.Helper()
	dir := t.TempDir()
	repo, err := storage.OpenRepository(dir, dir)
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
	host, text, err := net.SplitHostPort(server.address)
	if err != nil {
		t.Fatal(err)
	}
	port, err := strconv.Atoi(text)
	if err != nil {
		t.Fatal(err)
	}
	profile := model.Profile{Host: host, Port: port, User: "tester", Auth: []model.AuthMethod{{Kind: "password"}}}
	if err = repo.TrustHost(host, port, pin); err != nil {
		t.Fatal(err)
	}
	if err = vault.Set(profile.SecretID(), password); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	t.Cleanup(cancel)
	return manager, &Session{ctx: ctx, profile: profile}, profile, filepath.Join(dir, "known_hosts.toml")
}

func TestSSHRetainsKnownHostKeyAcrossNegotiationPreferences(t *testing.T) {
	ecdsaSigner, rsaSigner := hostKeySigners(t)
	certificate := &ssh.Certificate{Key: rsaSigner.PublicKey(), CertType: ssh.HostCert, ValidPrincipals: []string{"127.0.0.1"}, ValidBefore: ssh.CertTimeInfinity}
	if err := certificate.SignCert(rand.Reader, ecdsaSigner); err != nil {
		t.Fatal(err)
	}
	certificateSigner, err := ssh.NewCertSigner(certificate, rsaSigner)
	if err != nil {
		t.Fatal(err)
	}
	for _, test := range []struct {
		name        string
		key         ssh.PublicKey
		password    string
		changed     bool
		certificate bool
		attempts    int
	}{
		{name: "legacy-ed25519", password: "test-password", attempts: 3},
		{name: "existing-ecdsa", key: ecdsaSigner.PublicKey(), password: "test-password", attempts: 1},
		{name: "existing-rsa", key: rsaSigner.PublicKey(), password: "test-password", attempts: 2},
		{name: "changed-key", password: "test-password", changed: true, attempts: 4},
		{name: "authentication-failure", password: "incorrect-password", attempts: 3},
		{name: "legacy-ed25519-with-certificate", password: "test-password", certificate: true, attempts: 4},
		{name: "existing-rsa-certificate", key: certificateSigner.PublicKey(), password: "test-password", certificate: true, attempts: 1},
		{name: "changed-key-with-certificate", password: "test-password", changed: true, certificate: true, attempts: 5},
	} {
		t.Run(test.name, func(t *testing.T) {
			var authentication atomic.Int32
			server := startLoopbackConfigured(t, loopbackOptions{configure: func(config *ssh.ServerConfig) {
				config.AddHostKey(ecdsaSigner)
				config.AddHostKey(rsaSigner)
				if test.certificate {
					config.AddHostKey(certificateSigner)
				}
				original := config.PasswordCallback
				config.PasswordCallback = func(metadata ssh.ConnMetadata, password []byte) (*ssh.Permissions, error) {
					authentication.Add(1)
					return original(metadata, password)
				}
			}}, nil, nil)
			pin := server.fingerprint
			if test.key != nil {
				pin = ssh.FingerprintSHA256(test.key)
			}
			if test.changed {
				pin = "SHA256:" + strings.Repeat("A", 43)
			}
			manager, session, profile, path := knownHostHandshake(t, server, pin, test.password)
			before, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			attempts := 0
			client, closers, err := manager.handshake(session, profile, func(ctx context.Context, address string) (net.Conn, error) {
				attempts++
				return (&net.Dialer{}).DialContext(ctx, "tcp", address)
			})
			for i := len(closers) - 1; i >= 0; i-- {
				closers[i].Close()
			}
			if test.changed {
				if err == nil || !strings.Contains(err.Error(), "主机密钥变更") || authentication.Load() != 0 {
					t.Fatalf("换钥应在认证前拒绝: err=%v, auth=%d", err, authentication.Load())
				}
			} else if test.password != "test-password" {
				if err == nil || !strings.Contains(err.Error(), "authenticate") || authentication.Load() != 1 {
					t.Fatalf("认证失败不应重新协商: err=%v, auth=%d", err, authentication.Load())
				}
			} else if err != nil || client == nil || authentication.Load() != 1 {
				t.Fatalf("原密钥仍在服务器上却无法连接: err=%v, auth=%d", err, authentication.Load())
			}
			if attempts != test.attempts {
				t.Fatalf("握手次数未按密钥类型收敛: got=%d, want=%d", attempts, test.attempts)
			}
			after, readErr := os.ReadFile(path)
			if readErr != nil || !bytes.Equal(before, after) {
				t.Fatal("重新协商修改了原主机信任记录", readErr)
			}
		})
	}
}

func TestSSHHostKeyNegotiationCancellation(t *testing.T) {
	ecdsaSigner, _ := hostKeySigners(t)
	server := startLoopbackConfigured(t, loopbackOptions{configure: func(config *ssh.ServerConfig) { config.AddHostKey(ecdsaSigner) }}, nil, nil)
	manager, session, profile, _ := knownHostHandshake(t, server, server.fingerprint, "test-password")
	ctx, cancel := context.WithCancel(session.ctx)
	defer cancel()
	session.ctx = ctx
	second := make(chan struct{})
	done := make(chan error, 1)
	go func() {
		attempts := 0
		_, closers, err := manager.handshake(session, profile, func(ctx context.Context, address string) (net.Conn, error) {
			attempts++
			if attempts == 2 {
				close(second)
				<-ctx.Done()
				return nil, ctx.Err()
			}
			return (&net.Dialer{}).DialContext(ctx, "tcp", address)
		})
		for i := len(closers) - 1; i >= 0; i-- {
			closers[i].Close()
		}
		done <- err
	}()
	select {
	case <-second:
	case err := <-done:
		t.Fatalf("未进入兼容协商: %v", err)
	case <-time.After(time.Second):
		t.Fatal("兼容协商没有开始")
	}
	cancel()
	select {
	case err := <-done:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("取消原因被覆盖: %v", err)
		}
	case <-time.After(time.Second):
		t.Fatal("取消后协商未结束")
	}
}

func TestProxyJumpRetainsKnownHostKeysAcrossNegotiationPreferences(t *testing.T) {
	ecdsaSigner, _ := hostKeySigners(t)
	options := loopbackOptions{configure: func(config *ssh.ServerConfig) { config.AddHostKey(ecdsaSigner) }}
	target := startLoopbackConfigured(t, options, nil, nil)
	jump := startLoopbackConfigured(t, options, nil, nil)
	manager, repo := newLoopbackManager(t, target, "tester")
	profile, err := repo.Profile("loopback")
	if err != nil {
		t.Fatal(err)
	}
	for _, server := range []loopback{target, jump} {
		host, text, err := net.SplitHostPort(server.address)
		if err != nil {
			t.Fatal(err)
		}
		port, err := strconv.Atoi(text)
		if err != nil {
			t.Fatal(err)
		}
		if err = repo.TrustHost(host, port, server.fingerprint); err != nil {
			t.Fatal(err)
		}
		identity := model.Profile{Host: host, Port: port, User: "tester"}
		if err = manager.vault.Set(identity.SecretID(), "test-password"); err != nil {
			t.Fatal(err)
		}
	}
	profile.ProxyJump = "tester@" + jump.address
	if _, err = repo.SaveProfile(context.Background(), model.SaveProfileRequest{ExpectedRevision: repo.Snapshot().Revision, OriginalName: profile.Name, Profile: profile}); err != nil {
		t.Fatal(err)
	}
	opened, err := manager.Connect(profile.Name)
	if err != nil {
		t.Fatal(err)
	}
	connected := waitConnected(t, manager, opened, false)
	if _, err = manager.ReadDirectory(context.Background(), connected.ID, connected.Generation, remoteTestPath(target.root)); err != nil {
		t.Fatal("原指纹匹配后跳板SFTP不可用", err)
	}
}

func TestProxyJumpChangedHostKeyDoesNotRetryTarget(t *testing.T) {
	var jumpAttempts, targetAttempts atomic.Int32
	target := startLoopbackConfigured(t, loopbackOptions{transport: func(conn net.Conn) net.Conn {
		targetAttempts.Add(1)
		return conn
	}}, nil, nil)
	jump := startLoopbackConfigured(t, loopbackOptions{transport: func(conn net.Conn) net.Conn {
		jumpAttempts.Add(1)
		return conn
	}}, nil, nil)
	manager, repo := newLoopbackManager(t, target, "tester")
	profile, err := repo.Profile("loopback")
	if err != nil {
		t.Fatal(err)
	}
	host, text, err := net.SplitHostPort(jump.address)
	if err != nil {
		t.Fatal(err)
	}
	port, err := strconv.Atoi(text)
	if err != nil {
		t.Fatal(err)
	}
	if err = repo.TrustHost(host, port, "SHA256:"+strings.Repeat("A", 43)); err != nil {
		t.Fatal(err)
	}
	profile.ProxyJump = "tester@" + jump.address
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_, closers, err := manager.dial(&Session{ctx: ctx, profile: profile})
	for i := len(closers) - 1; i >= 0; i-- {
		closers[i].Close()
	}
	if err == nil || !strings.Contains(err.Error(), "主机密钥变更") || jumpAttempts.Load() != 2 || targetAttempts.Load() != 0 {
		t.Fatalf("跳板校验失败应停止外层重试: err=%v, jump=%d, target=%d", err, jumpAttempts.Load(), targetAttempts.Load())
	}
}
