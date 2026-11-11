package session

import (
	"bufio"
	"context"
	"crypto/tls"
	"encoding/base64"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/kevinburke/ssh_config"
	"golang.org/x/crypto/ssh"
	"golang.org/x/crypto/ssh/agent"
	"golang.org/x/net/proxy"
	"kitonyterms/internal/model"
)

func (m *Manager) dial(s *Session) (*ssh.Client, []io.Closer, error) {
	p := s.profile
	if m.repo.Snapshot().Config.Settings.UseSSHConfig {
		if p.VaultID == "" {
			p.VaultID = p.SecretID()
		}
		var err error
		p, err = mergeSSHConfig(p, ssh_config.DefaultUserSettings)
		if err != nil {
			return nil, nil, err
		}
	}
	closers := []io.Closer{}
	ok := false
	defer func() {
		if !ok {
			for i := len(closers) - 1; i >= 0; i-- {
				closers[i].Close()
			}
		}
	}()
	dial := func(ctx context.Context, address string) (net.Conn, error) { return dialProxy(ctx, p.Proxy, address) }
	if p.ProxyJump != "" {
		jump := model.Profile{Name: "ProxyJump", User: p.User, Port: 22, Auth: p.Auth, Proxy: model.Proxy{Kind: "direct"}}
		endpoint := p.ProxyJump
		if at := strings.LastIndex(endpoint, "@"); at >= 0 {
			jump.User = endpoint[:at]
			endpoint = endpoint[at+1:]
		}
		if h, port, e := net.SplitHostPort(endpoint); e == nil {
			jump.Host = h
			jump.Port, _ = strconv.Atoi(port)
		} else {
			jump.Host = endpoint
		}
		if jump.Host == "" || jump.Port < 1 || jump.Port > 65535 {
			return nil, nil, fmt.Errorf("ProxyJump 地址无效")
		}
		proxyDial := dial
		dial = func(ctx context.Context, address string) (net.Conn, error) {
			client, cs, err := m.handshakeContext(ctx, s, jump, proxyDial)
			if err != nil {
				return nil, err
			}
			closers = append(closers, cs...)
			return dialJump(ctx, client, address)
		}
	}
	client, cs, err := m.handshake(s, p, dial)
	if err != nil {
		return nil, nil, err
	}
	closers = append(closers, cs...)
	ok = true
	return client, closers, nil
}

type sshSettings interface {
	GetStrict(alias, key string) (string, error)
	GetAllStrict(alias, key string) ([]string, error)
}

func mergeSSHConfig(p model.Profile, settings sshSettings) (model.Profile, error) {
	alias := p.Host
	values := map[string]string{}
	for _, key := range []string{"HostName", "Port", "User", "ProxyJump"} {
		value, err := settings.GetStrict(alias, key)
		if err != nil {
			return p, fmt.Errorf("解析 SSH config 失败: %w", err)
		}
		values[key] = value
	}
	if values["HostName"] != "" {
		p.Host = values["HostName"]
	}
	if p.Port == 22 {
		if port, err := strconv.Atoi(values["Port"]); err == nil && port > 0 && port <= 65535 {
			p.Port = port
		}
	}
	if p.User == "" {
		p.User = values["User"]
	}
	identities, err := settings.GetAllStrict(alias, "IdentityFile")
	if err != nil {
		return p, fmt.Errorf("解析 SSH 身份配置失败: %w", err)
	}
	for _, key := range identities {
		if key == "" || key == "none" {
			continue
		}
		keyPath, err := expandKeyPath(key)
		if err != nil {
			return p, err
		}
		info, err := os.Stat(keyPath)
		// OpenSSH 的自动身份候选可以不存在，不影响其它认证方法。
		if os.IsNotExist(err) {
			continue
		}
		if err != nil {
			return p, fmt.Errorf("检查 SSH 身份文件失败: %w", err)
		}
		if !info.Mode().IsRegular() {
			return p, fmt.Errorf("SSH 身份文件必须为普通文件")
		}
		found := false
		for _, auth := range p.Auth {
			if auth.Kind == "public_key" && auth.KeyPath == key {
				found = true
			}
		}
		if !found {
			p.Auth = append(p.Auth, model.AuthMethod{Kind: "public_key", KeyPath: key})
		}
	}
	if p.ProxyJump == "" && values["ProxyJump"] != "none" {
		p.ProxyJump = values["ProxyJump"]
	}
	return p, nil
}

func expandKeyPath(keyPath string) (string, error) {
	if strings.HasPrefix(keyPath, "~/") {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", err
		}
		return filepath.Join(home, keyPath[2:]), nil
	}
	return keyPath, nil
}
func (m *Manager) handshakeAttempt(ctx context.Context, s *Session, p model.Profile, dial func(context.Context, string) (net.Conn, error), algorithms []string) (*ssh.Client, []io.Closer, error) {
	if p.User == "" {
		return nil, nil, fmt.Errorf("SSH 用户名未设置")
	}
	conn, err := dial(ctx, p.Address())
	if err != nil {
		return nil, nil, &handshakeDialError{cause: err}
	}
	closers := []io.Closer{conn}
	success := false
	defer func() {
		if !success {
			for _, c := range closers {
				c.Close()
			}
		}
	}()
	stop := context.AfterFunc(ctx, func() { conn.Close() })
	defer stop()
	methods := []ssh.AuthMethod{}
	secret, hasSecret := m.vault.Get(p.SecretID())
	for _, auth := range p.Auth {
		switch auth.Kind {
		case "password":
			methods = append(methods, ssh.PasswordCallback(func() (string, error) {
				if hasSecret {
					return secret, nil
				}
				a, e := m.ask(s, model.Challenge{Kind: "password", Host: p.Host, Port: p.Port, Prompts: []model.Prompt{{Text: p.User + " 的密码", Echo: false}}})
				if e != nil {
					return "", e
				}
				if len(a.Answers) != 1 {
					return "", fmt.Errorf("密码认证已取消")
				}
				return a.Answers[0], nil
			}))
		case "keyboard_interactive":
			methods = append(methods, ssh.KeyboardInteractive(func(user, instruction string, questions []string, echo []bool) ([]string, error) {
				prompts := make([]model.Prompt, len(questions))
				for i, q := range questions {
					prompts[i] = model.Prompt{Text: q, Echo: echo[i]}
				}
				a, e := m.ask(s, model.Challenge{Kind: "keyboard_interactive", Host: p.Host, Port: p.Port, Message: instruction, Prompts: prompts})
				if e != nil {
					return nil, e
				}
				if len(a.Answers) != len(questions) {
					return nil, fmt.Errorf("交互认证已取消")
				}
				return a.Answers, nil
			}))
		case "public_key":
			keyPath, e := expandKeyPath(auth.KeyPath)
			if e != nil {
				return nil, nil, e
			}
			data, e := os.ReadFile(keyPath)
			if e != nil {
				return nil, nil, e
			}
			signer, e := ssh.ParsePrivateKey(data)
			if _, encrypted := e.(*ssh.PassphraseMissingError); encrypted {
				passphrase := secret
				if !hasSecret {
					a, askErr := m.ask(s, model.Challenge{Kind: "key_passphrase", Host: p.Host, Port: p.Port, Prompts: []model.Prompt{{Text: "私钥口令", Echo: false}}})
					if askErr != nil {
						clear(data)
						return nil, nil, askErr
					}
					if len(a.Answers) != 1 {
						clear(data)
						return nil, nil, fmt.Errorf("私钥认证已取消")
					}
					passphrase = a.Answers[0]
				}
				signer, e = ssh.ParsePrivateKeyWithPassphrase(data, []byte(passphrase))
			}
			clear(data)
			if e != nil {
				return nil, nil, fmt.Errorf("读取私钥失败: %w", e)
			}
			methods = append(methods, ssh.PublicKeys(signer))
		case "agent":
			agentConn, e := m.agentDial(ctx)
			if e != nil {
				return nil, nil, e
			}
			closers = append(closers, agentConn)
			stopAgent := context.AfterFunc(ctx, func() { agentConn.Close() })
			defer stopAgent()
			methods = append(methods, ssh.PublicKeysCallback(agent.NewClient(agentConn).Signers))
		}
	}
	config := &ssh.ClientConfig{User: p.User, Auth: methods, Timeout: 30 * time.Second, HostKeyAlgorithms: algorithms, HostKeyCallback: func(_ string, _ net.Addr, key ssh.PublicKey) error {
		fingerprint := ssh.FingerprintSHA256(key)
		expected, trusted := m.repo.CheckHost(p.Host, p.Port, fingerprint)
		if trusted {
			return nil
		}
		if expected != "" {
			return &hostKeyChangedError{expected: expected, actual: fingerprint, algorithm: key.Type()}
		}
		a, e := m.ask(s, model.Challenge{Kind: "host_key", Host: p.Host, Port: p.Port, Fingerprint: fingerprint, Prompts: []model.Prompt{}, Message: "首次连接，请核对主机密钥"})
		if e != nil {
			return e
		}
		if !a.Trust {
			return fmt.Errorf("用户拒绝主机密钥")
		}
		return m.repo.TrustHost(p.Host, p.Port, fingerprint)
	}}
	sshConn, chans, reqs, err := ssh.NewClientConn(conn, p.Address(), config)
	if err != nil {
		return nil, nil, err
	}
	client := ssh.NewClient(sshConn, chans, reqs)
	closers = append(closers, client)
	success = true
	return client, closers, nil
}

type bufferedConn struct {
	net.Conn
	reader *bufio.Reader
}

func (c *bufferedConn) Read(p []byte) (int, error) { return c.reader.Read(p) }
func dialProxy(ctx context.Context, p model.Proxy, address string) (net.Conn, error) {
	dialer := &net.Dialer{Timeout: 30 * time.Second, KeepAlive: 30 * time.Second}
	if p.Kind == "" || p.Kind == "direct" {
		return dialer.DialContext(ctx, "tcp", address)
	}
	var endpoint *url.URL
	if p.Kind == "system" {
		for _, key := range []string{"ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"} {
			if value := os.Getenv(key); value != "" {
				var err error
				endpoint, err = url.Parse(value)
				if err != nil {
					return nil, fmt.Errorf("系统代理 URL 无效")
				}
				break
			}
		}
		if endpoint == nil {
			return dialer.DialContext(ctx, "tcp", address)
		}
	} else {
		scheme := p.Kind
		if scheme == "http" {
			scheme = "http"
		}
		endpoint = &url.URL{Scheme: scheme, Host: net.JoinHostPort(p.Host, strconv.Itoa(p.Port))}
		if p.Username != "" {
			endpoint.User = url.User(p.Username)
		}
	}
	switch endpoint.Scheme {
	case "socks5", "socks5h":
		var auth *proxy.Auth
		if endpoint.User != nil {
			password, _ := endpoint.User.Password()
			auth = &proxy.Auth{User: endpoint.User.Username(), Password: password}
		}
		pd, err := proxy.SOCKS5("tcp", endpoint.Host, auth, dialer)
		if err != nil {
			return nil, err
		}
		contextDialer, ok := pd.(proxy.ContextDialer)
		if !ok {
			return nil, fmt.Errorf("代理拨号器不支持取消")
		}
		return contextDialer.DialContext(ctx, "tcp", address)
	case "http", "https":
		proxyAddress := endpoint.Host
		if endpoint.Port() == "" {
			port := "80"
			if endpoint.Scheme == "https" {
				port = "443"
			}
			proxyAddress = net.JoinHostPort(endpoint.Hostname(), port)
		}
		conn, err := dialer.DialContext(ctx, "tcp", proxyAddress)
		if err != nil {
			return nil, err
		}
		stop := context.AfterFunc(ctx, func() { conn.Close() })
		defer stop()
		conn.SetDeadline(time.Now().Add(30 * time.Second))
		if endpoint.Scheme == "https" {
			secure := tls.Client(conn, &tls.Config{ServerName: endpoint.Hostname(), MinVersion: tls.VersionTLS12})
			if err = secure.HandshakeContext(ctx); err != nil {
				conn.Close()
				return nil, err
			}
			conn = secure
		}
		req := &http.Request{Method: http.MethodConnect, URL: &url.URL{Opaque: address}, Host: address, Header: make(http.Header)}
		if endpoint.User != nil {
			password, _ := endpoint.User.Password()
			req.Header.Set("Proxy-Authorization", "Basic "+base64.StdEncoding.EncodeToString([]byte(endpoint.User.Username()+":"+password)))
		}
		if err = req.Write(conn); err != nil {
			conn.Close()
			return nil, err
		}
		reader := bufio.NewReaderSize(conn, 16<<10)
		response, err := http.ReadResponse(reader, req)
		if err != nil {
			conn.Close()
			return nil, err
		}
		if response.StatusCode != 200 {
			conn.Close()
			return nil, fmt.Errorf("HTTP 代理拒绝连接: %d", response.StatusCode)
		}
		if err = conn.SetDeadline(time.Time{}); err != nil {
			conn.Close()
			return nil, err
		}
		return &bufferedConn{Conn: conn, reader: reader}, nil
	default:
		return nil, fmt.Errorf("不支持的代理协议")
	}
}
