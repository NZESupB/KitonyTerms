package update

import (
	"fmt"
	"net"
	"net/http"
	"net/url"
	"os"
	"strconv"
	"strings"
	"time"
	"unicode"

	"golang.org/x/net/http/httpproxy"
	"kitonyterms/internal/model"
)

func validateProxy(p model.UpdateProxy) error {
	switch p.Kind {
	case "system", "direct":
		if p.Host != "" || p.Port != 0 || p.Username != "" {
			return fmt.Errorf("非显式代理不能保存主机或用户名")
		}
		return nil
	case "http", "https", "socks5":
	default:
		return fmt.Errorf("更新代理类型无效")
	}
	if p.Host == "" || len(p.Host) > 255 || strings.TrimSpace(p.Host) != p.Host || p.Port < 1 || p.Port > 65535 {
		return fmt.Errorf("更新代理地址或端口无效")
	}
	for _, c := range p.Host {
		if unicode.IsSpace(c) || unicode.IsControl(c) || strings.ContainsRune("/@?#\\", c) {
			return fmt.Errorf("更新代理主机无效")
		}
	}
	host := p.Host
	if strings.ContainsAny(host, "[]") {
		if !strings.HasPrefix(host, "[") || !strings.HasSuffix(host, "]") {
			return fmt.Errorf("更新代理IPv6地址无效")
		}
		host = host[1 : len(host)-1]
		if net.ParseIP(host) == nil || !strings.Contains(host, ":") {
			return fmt.Errorf("更新代理IPv6地址无效")
		}
	}
	if strings.Contains(host, ":") && net.ParseIP(host) == nil {
		return fmt.Errorf("更新代理IPv6地址无效")
	}
	if len(p.Username) > 128 || strings.TrimSpace(p.Username) != p.Username {
		return fmt.Errorf("更新代理用户名无效")
	}
	for _, c := range p.Username {
		if unicode.IsControl(c) {
			return fmt.Errorf("更新代理用户名无效")
		}
	}
	return nil
}

func proxyFunction(p model.UpdateProxy) (func(*http.Request) (*url.URL, error), error) {
	if err := validateProxy(p); err != nil {
		return nil, err
	}
	if p.Kind == "direct" {
		return nil, nil
	}
	if p.Kind == "system" {
		config := httpproxy.FromEnvironment()
		all := os.Getenv("ALL_PROXY")
		if all == "" {
			all = os.Getenv("all_proxy")
		}
		if config.HTTPProxy == "" {
			config.HTTPProxy = all
		}
		if config.HTTPSProxy == "" {
			config.HTTPSProxy = all
		}
		resolve := config.ProxyFunc()
		return func(r *http.Request) (*url.URL, error) { return resolve(r.URL) }, nil
	}
	host := strings.TrimSuffix(strings.TrimPrefix(p.Host, "["), "]")
	scheme := p.Kind
	if scheme == "socks5" {
		scheme = "socks5h"
	}
	endpoint := &url.URL{Scheme: scheme, Host: net.JoinHostPort(host, strconv.Itoa(p.Port))}
	if p.Username != "" {
		endpoint.User = url.UserPassword(p.Username, "")
	}
	return http.ProxyURL(endpoint), nil
}

func newClient(p model.UpdateProxy) (*releaseClient, error) {
	proxy, err := proxyFunction(p)
	if err != nil {
		return nil, err
	}
	transport := &http.Transport{
		Proxy:               proxy,
		DialContext:         (&net.Dialer{Timeout: 10 * time.Second, KeepAlive: 30 * time.Second}).DialContext,
		TLSHandshakeTimeout: 10 * time.Second, ResponseHeaderTimeout: 30 * time.Second,
		IdleConnTimeout: 30 * time.Second, ExpectContinueTimeout: time.Second,
		MaxIdleConns: 4, MaxIdleConnsPerHost: 2, MaxConnsPerHost: 4,
		DisableCompression: true,
	}
	client := &http.Client{Transport: transport, Timeout: 15 * time.Minute, CheckRedirect: func(req *http.Request, via []*http.Request) error {
		if len(via) >= 5 || !trustedURL(req.URL) {
			return &failure{Kind: "redirect"}
		}
		return nil
	}}
	return &releaseClient{http: client, transport: transport, base: officialAPI, idle: 30 * time.Second}, nil
}
