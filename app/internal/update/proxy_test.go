package update

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/binary"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"kitonyterms/internal/model"
)

func TestHTTPAndHTTPSProxyTunnel(t *testing.T) {
	for _, kind := range []string{"http", "https"} {
		t.Run(kind, func(t *testing.T) {
			f := newFeed()
			target := httptest.NewTLSServer(f)
			defer target.Close()
			var tunnels atomic.Int64
			proxy := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.Method != "CONNECT" || r.Host != target.Listener.Addr().String() || r.Header.Get("Proxy-Authorization") != "Basic Zml4dHVyZS11c2VyOg==" {
					w.WriteHeader(407)
					return
				}
				remote, err := net.DialTimeout("tcp", r.Host, time.Second)
				if err != nil {
					w.WriteHeader(502)
					return
				}
				connection, buffer, err := w.(http.Hijacker).Hijack()
				if err != nil {
					remote.Close()
					return
				}
				defer remote.Close()
				defer connection.Close()
				tunnels.Add(1)
				buffer.WriteString("HTTP/1.1 200 Connection Established\r\n\r\n")
				buffer.Flush()
				go func() { io.Copy(remote, buffer); remote.Close() }()
				io.Copy(connection, remote)
			}))
			if kind == "https" {
				proxy.StartTLS()
			} else {
				proxy.Start()
			}
			defer proxy.Close()
			endpoint, _ := url.Parse(proxy.URL)
			port, _ := strconv.Atoi(endpoint.Port())
			client, err := newClient(model.UpdateProxy{Kind: kind, Host: endpoint.Hostname(), Port: port, Username: "fixture-user"})
			if err != nil {
				t.Fatal(err)
			}
			defer client.close()
			pool := x509.NewCertPool()
			pool.AddCert(target.Certificate())
			if kind == "https" {
				pool.AddCert(proxy.Certificate())
			}
			client.transport.TLSClientConfig = &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}
			client.base = target.URL
			ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
			defer cancel()
			candidate, err := client.check(ctx, testBuild(), false)
			if err != nil {
				t.Fatal(err)
			}
			root, err := openRoot(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			defer root.Close()
			if err = client.download(ctx, root, *candidate, func(uint64, uint64) {}); err != nil {
				t.Fatal(err)
			}
			if tunnels.Load() == 0 {
				t.Fatal("检查与下载未经过显式代理")
			}
		})
	}
}

func TestProxyAuthErrorDoesNotExposeEndpoint(t *testing.T) {
	proxy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(407) }))
	defer proxy.Close()
	u, _ := url.Parse(proxy.URL)
	port, _ := strconv.Atoi(u.Port())
	client, err := newClient(model.UpdateProxy{Kind: "http", Host: u.Hostname(), Port: port, Username: "fixture-user-marker"})
	if err != nil {
		t.Fatal(err)
	}
	defer client.close()
	_, err = client.check(context.Background(), testBuild(), false)
	requireFailure(t, err, "proxy_auth")
	if strings.Contains(err.Error(), "fixture-user-marker") || strings.Contains(err.Error(), proxy.URL) {
		t.Fatal("错误暴露了代理端点")
	}
	for _, host := range []string{"[]", "[host]", "[::1]suffix"} {
		if validateProxy(model.UpdateProxy{Kind: "http", Host: host, Port: 80}) == nil {
			t.Fatal("接受伪IPv6代理")
		}
	}
}

func TestSOCKS5ProxyUsesRemoteDNSAndEmptyPassword(t *testing.T) {
	feed := newFeed()
	target := httptest.NewServer(feed)
	defer target.Close()
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	var workers sync.WaitGroup
	var authenticated, remoteDNS atomic.Bool
	workers.Add(1)
	go func() {
		defer workers.Done()
		for {
			connection, err := listener.Accept()
			if err != nil {
				return
			}
			workers.Add(1)
			go func() {
				defer workers.Done()
				defer connection.Close()
				connection.SetDeadline(time.Now().Add(5 * time.Second))
				var header [2]byte
				if _, err := io.ReadFull(connection, header[:]); err != nil || header[0] != 5 {
					return
				}
				methods := make([]byte, int(header[1]))
				if _, err := io.ReadFull(connection, methods); err != nil || !bytes.Contains(methods, []byte{2}) {
					return
				}
				connection.Write([]byte{5, 2})
				if _, err := io.ReadFull(connection, header[:]); err != nil || header[0] != 1 {
					return
				}
				username := make([]byte, int(header[1]))
				if _, err := io.ReadFull(connection, username); err != nil {
					return
				}
				var length [1]byte
				if _, err := io.ReadFull(connection, length[:]); err != nil {
					return
				}
				password := make([]byte, int(length[0]))
				if _, err := io.ReadFull(connection, password); err != nil {
					return
				}
				if string(username) != "fixture-user" || len(password) != 0 {
					return
				}
				authenticated.Store(true)
				connection.Write([]byte{1, 0})
				var request [4]byte
				if _, err := io.ReadFull(connection, request[:]); err != nil || request != [4]byte{5, 1, 0, 3} {
					return
				}
				if _, err := io.ReadFull(connection, length[:]); err != nil {
					return
				}
				host := make([]byte, int(length[0]))
				if _, err := io.ReadFull(connection, host); err != nil {
					return
				}
				var portBytes [2]byte
				if _, err := io.ReadFull(connection, portBytes[:]); err != nil {
					return
				}
				if string(host) != "fixture.invalid" || binary.BigEndian.Uint16(portBytes[:]) == 0 {
					return
				}
				remoteDNS.Store(true)
				remote, err := net.DialTimeout("tcp", target.Listener.Addr().String(), time.Second)
				if err != nil {
					return
				}
				defer remote.Close()
				connection.Write([]byte{5, 0, 0, 1, 127, 0, 0, 1, 0, 0})
				go func() { io.Copy(remote, connection); remote.Close() }()
				io.Copy(connection, remote)
			}()
		}
	}()
	defer func() { listener.Close(); workers.Wait() }()
	address := listener.Addr().(*net.TCPAddr)
	client, err := newClient(model.UpdateProxy{Kind: "socks5", Host: "127.0.0.1", Port: address.Port, Username: "fixture-user"})
	if err != nil {
		t.Fatal(err)
	}
	defer client.close()
	targetURL, _ := url.Parse(target.URL)
	client.base = "http://fixture.invalid:" + targetURL.Port()
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	candidate, err := client.check(ctx, testBuild(), false)
	if err != nil {
		t.Fatal(err)
	}
	root, err := openRoot(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	if err = client.download(ctx, root, *candidate, func(uint64, uint64) {}); err != nil {
		t.Fatal(err)
	}
	if !authenticated.Load() || !remoteDNS.Load() {
		t.Fatal("SOCKS5用户名空密码或远端解析未生效")
	}
}

func TestUntrustedOrWrongTLSIdentityIsRejected(t *testing.T) {
	for _, name := range []string{"untrusted", "wrong-name"} {
		t.Run(name, func(t *testing.T) {
			server := httptest.NewTLSServer(newFeed())
			defer server.Close()
			client, err := newClient(model.UpdateProxy{Kind: "direct"})
			if err != nil {
				t.Fatal(err)
			}
			defer client.close()
			pool := x509.NewCertPool()
			settings := &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}
			if name == "wrong-name" {
				pool.AddCert(server.Certificate())
				settings.ServerName = "mismatch.invalid"
			}
			client.transport.TLSClientConfig = settings
			client.base = server.URL
			_, err = client.check(context.Background(), testBuild(), false)
			requireFailure(t, err, "tls")
		})
	}
}
