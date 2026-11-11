package syncconfig

import (
	"context"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"golang.org/x/crypto/chacha20poly1305"
	"golang.org/x/net/netutil"
	"kitonyterms/internal/model"
)

const authDomain = "kitonyterms.lan.auth.v2"
const encryptionDomain = "kitonyterms.lan.encryption.v2"
const responseDomain = "kitonyterms.lan.response.v2"
const pairingAlphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
const configPath = "/v2/config"
const ackPath = "/v2/ack"

type ShareInfo struct {
	URL           string `json:"url"`
	PairingCode   string `json:"pairingCode"`
	ExpiresAtUnix int64  `json:"expiresAtUnix"`
}
type Share struct {
	mu         sync.Mutex
	info       ShareInfo
	server     *http.Server
	cancel     context.CancelFunc
	done       chan struct{}
	state      string
	auth       []byte
	id         []byte
	nonce      []byte
	ciphertext []byte
	nonces     map[string]bool
	order      []string
	delivery   chan struct{}
}

func normalizeCode(code string) string {
	return strings.Map(func(r rune) rune {
		switch r {
		case ' ', '\t', '\r', '\n', '-':
			return -1
		}
		r = []rune(strings.ToUpper(string(r)))[0]
		switch r {
		case 'O':
			return '0'
		case 'I', 'L':
			return '1'
		}
		return r
	}, code)
}
func keys(code string) ([]byte, []byte, error) {
	code = normalizeCode(code)
	if len(code) != 26 {
		return nil, nil, fmt.Errorf("配对码必须为 26 位")
	}
	for _, r := range code {
		if !strings.ContainsRune(pairingAlphabet, r) {
			return nil, nil, fmt.Errorf("配对码字符无效")
		}
	}
	derive := func(domain string) []byte {
		mac := hmac.New(sha256.New, []byte(code))
		mac.Write([]byte(domain))
		return mac.Sum(nil)
	}
	return derive(authDomain), derive(encryptionDomain), nil
}
func randomBytes(n int) ([]byte, error) {
	data := make([]byte, n)
	_, err := rand.Read(data)
	return data, err
}
func requestMAC(key []byte, method, path string, nonce, id []byte) []byte {
	mac := hmac.New(sha256.New, key)
	for _, part := range [][]byte{[]byte(authDomain), []byte(method), []byte(path), nonce} {
		mac.Write(part)
		mac.Write([]byte{0})
	}
	mac.Write(id)
	return mac.Sum(nil)
}
func aad(id []byte) []byte { return append(append([]byte(responseDomain), 0), id...) }
func hexHeader(h http.Header, name string, size int) ([]byte, error) {
	value, err := hex.DecodeString(h.Get(name))
	if err != nil || len(value) != size {
		return nil, fmt.Errorf("LAN 响应头无效")
	}
	return value, nil
}
func addAuthentication(req *http.Request, key, id []byte) error {
	nonce, err := randomBytes(16)
	if err != nil {
		return err
	}
	req.Header.Set("X-KitonyTerms-Protocol", "2")
	req.Header.Set("X-KitonyTerms-Nonce", hex.EncodeToString(nonce))
	req.Header.Set("X-KitonyTerms-Auth", hex.EncodeToString(requestMAC(key, req.Method, req.URL.Path, nonce, id)))
	if id != nil {
		req.Header.Set("X-KitonyTerms-Delivery", hex.EncodeToString(id))
	}
	return nil
}
func (s *Share) authenticated(req *http.Request, id []byte) bool {
	if req.Header.Get("X-KitonyTerms-Protocol") != "2" {
		return false
	}
	nonce, err := hexHeader(req.Header, "X-KitonyTerms-Nonce", 16)
	if err != nil {
		return false
	}
	mac, err := hexHeader(req.Header, "X-KitonyTerms-Auth", 32)
	if err != nil {
		return false
	}
	if id != nil {
		actual, err := hexHeader(req.Header, "X-KitonyTerms-Delivery", 16)
		if err != nil || !hmac.Equal(actual, id) {
			return false
		}
	}
	return hmac.Equal(mac, requestMAC(s.auth, req.Method, req.URL.Path, nonce, id))
}
func (s *Share) claim(req *http.Request) bool {
	key := req.Method + strings.ToLower(req.Header.Get("X-KitonyTerms-Nonce"))
	if s.nonces[key] {
		return false
	}
	s.nonces[key] = true
	s.order = append(s.order, key)
	if len(s.order) > 1024 {
		delete(s.nonces, s.order[0])
		s.order = s.order[1:]
	}
	return true
}
func (s *Share) ServeHTTP(w http.ResponseWriter, req *http.Request) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Connection", "close")
	w.Header().Set("X-KitonyTerms-Protocol", "2")
	if req.URL.RawQuery != "" {
		http.Error(w, "invalid query", 400)
		return
	}
	isGet := req.Method == http.MethodGet && req.URL.Path == configPath
	isAck := req.Method == http.MethodPost && req.URL.Path == ackPath
	if !isGet && !isAck {
		http.NotFound(w, req)
		return
	}
	var expectedID []byte
	if isAck {
		expectedID = s.id
	}
	if !s.authenticated(req, expectedID) {
		w.Header().Set("WWW-Authenticate", "KitonyTerms-HMAC")
		http.Error(w, "authentication required", 401)
		return
	}
	s.mu.Lock()
	if isAck && s.state == "delivering" {
		delivery := s.delivery
		s.mu.Unlock()
		select {
		case <-delivery:
		case <-req.Context().Done():
			return
		}
		s.mu.Lock()
	}
	if !s.claim(req) {
		s.mu.Unlock()
		http.Error(w, "request replayed", 409)
		return
	}
	if s.state == "consumed" {
		s.mu.Unlock()
		http.Error(w, "share consumed", 410)
		return
	}
	if s.state == "delivering" || s.state == "acking" {
		s.mu.Unlock()
		http.Error(w, "share busy", 409)
		return
	}
	if isAck && s.state != "awaiting" {
		s.mu.Unlock()
		http.Error(w, "nothing delivered", 409)
		return
	}
	previous := s.state
	if isGet {
		s.state = "delivering"
		s.delivery = make(chan struct{})
	} else {
		s.state = "acking"
	}
	s.mu.Unlock()
	if isGet {
		w.Header().Set("Content-Type", "application/octet-stream")
		w.Header().Set("Content-Length", strconv.Itoa(len(s.ciphertext)))
		w.Header().Set("X-KitonyTerms-Delivery", hex.EncodeToString(s.id))
		w.Header().Set("X-KitonyTerms-Response-Nonce", hex.EncodeToString(s.nonce))
		_, err := w.Write(s.ciphertext)
		if err == nil {
			err = http.NewResponseController(w).Flush()
		}
		s.mu.Lock()
		if err == nil {
			s.state = "awaiting"
		} else {
			s.state = previous
		}
		close(s.delivery)
		s.mu.Unlock()
		return
	}
	w.Header().Set("Content-Length", "0")
	w.WriteHeader(http.StatusNoContent)
	err := http.NewResponseController(w).Flush()
	s.mu.Lock()
	if err == nil {
		s.state = "consumed"
	} else {
		s.state = "awaiting"
	}
	s.mu.Unlock()
	if err == nil {
		go s.Stop()
	}
}
func LANAddress() (string, error) {
	interfaces, err := net.Interfaces()
	if err != nil {
		return "", err
	}
	for _, iface := range interfaces {
		if iface.Flags&net.FlagUp == 0 || iface.Flags&net.FlagLoopback != 0 || iface.Flags&net.FlagPointToPoint != 0 {
			continue
		}
		name := strings.ToLower(iface.Name)
		if strings.HasPrefix(name, "utun") || strings.HasPrefix(name, "tun") || strings.HasPrefix(name, "wg") {
			continue
		}
		addresses, err := iface.Addrs()
		if err != nil {
			continue
		}
		for _, address := range addresses {
			ip, _, err := net.ParseCIDR(address.String())
			if err == nil && ip.To4() != nil && ip.IsPrivate() {
				return ip.String(), nil
			}
		}
	}
	return "", fmt.Errorf("没有可用的局域网 IPv4 地址")
}
func StartShare(ctx context.Context, config model.Config, bindIP string, ttl time.Duration) (*Share, error) {
	if ttl <= 0 || ttl > 10*time.Minute {
		return nil, fmt.Errorf("分享有效期应在 10 分钟以内")
	}
	if bindIP == "" {
		var err error
		bindIP, err = LANAddress()
		if err != nil {
			return nil, err
		}
	}
	ip := net.ParseIP(bindIP)
	if ip == nil || ip.IsUnspecified() {
		return nil, fmt.Errorf("需要明确的本机 IP 地址")
	}
	data, err := NewEnvelope(config).Encode()
	if err != nil {
		return nil, err
	}
	random, err := randomBytes(26)
	if err != nil {
		return nil, err
	}
	for i, value := range random {
		random[i] = pairingAlphabet[int(value)%32]
	}
	code := string(random)
	auth, encryption, err := keys(code)
	if err != nil {
		return nil, err
	}
	id, err := randomBytes(16)
	if err != nil {
		return nil, err
	}
	nonce, err := randomBytes(12)
	if err != nil {
		return nil, err
	}
	cipher, err := chacha20poly1305.New(encryption)
	clear(encryption)
	if err != nil {
		return nil, err
	}
	ciphertext := cipher.Seal(nil, nonce, data, aad(id))
	listener, err := net.Listen("tcp", net.JoinHostPort(ip.String(), "0"))
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(ctx, ttl)
	s := &Share{cancel: cancel, done: make(chan struct{}), delivery: make(chan struct{}), state: "available", auth: auth, id: id, nonce: nonce, ciphertext: ciphertext, nonces: map[string]bool{}}
	close(s.delivery)
	s.info = ShareInfo{URL: "http://" + listener.Addr().String() + configPath, PairingCode: code, ExpiresAtUnix: time.Now().Add(ttl).Unix()}
	s.server = &http.Server{Handler: s, ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 10 * time.Second, WriteTimeout: 10 * time.Second, IdleTimeout: time.Second, MaxHeaderBytes: 16 << 10}
	go func() { defer close(s.done); s.server.Serve(netutil.LimitListener(listener, 16)) }()
	go func() { <-ctx.Done(); s.server.Close() }()
	return s, nil
}
func (s *Share) Info() ShareInfo { return s.info }
func (s *Share) Stop()           { s.cancel(); s.server.Close(); <-s.done }
func (s *Share) Finished() bool {
	select {
	case <-s.done:
		return true
	default:
		return false
	}
}
func ImportLAN(ctx context.Context, address, code string, commit func(model.Config) error) error {
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	if strings.HasPrefix(strings.TrimSpace(address), "kitonyterms://lan-share?") {
		payload, err := url.Parse(strings.TrimSpace(address))
		if err != nil || payload.Query().Get("v") != "2" {
			return fmt.Errorf("分享负载无效")
		}
		address = payload.Query().Get("url")
		code = payload.Query().Get("code")
	}
	u, err := httpURL(address)
	if err != nil {
		return err
	}
	if u.Path != configPath || u.RawQuery != "" {
		return fmt.Errorf("分享路径必须为 /v2/config")
	}
	auth, encryption, err := keys(code)
	if err != nil {
		return err
	}
	defer clear(auth)
	defer clear(encryption)
	c := client(true)
	defer c.CloseIdleConnections()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u.String(), nil)
	if err != nil {
		return err
	}
	if err = addAuthentication(req, auth, nil); err != nil {
		return err
	}
	response, err := c.Do(req)
	if err != nil {
		return fmt.Errorf("LAN 下载失败")
	}
	defer response.Body.Close()
	if err = status(response); err != nil {
		return err
	}
	if response.Header.Get("X-KitonyTerms-Protocol") != "2" {
		return fmt.Errorf("LAN 响应版本无效")
	}
	id, err := hexHeader(response.Header, "X-KitonyTerms-Delivery", 16)
	if err != nil {
		return err
	}
	nonce, err := hexHeader(response.Header, "X-KitonyTerms-Response-Nonce", 12)
	if err != nil {
		return err
	}
	data, err := readBounded(response, MaxEnvelopeBytes+16)
	if err != nil {
		return err
	}
	cipher, err := chacha20poly1305.New(encryption)
	if err != nil {
		return err
	}
	plain, err := cipher.Open(nil, nonce, data, aad(id))
	if err != nil {
		return fmt.Errorf("LAN 响应认证失败")
	}
	envelope, err := Decode(plain)
	if err != nil {
		return err
	}
	if err = commit(envelope.Config); err != nil {
		return err
	}
	u.Path = ackPath
	for attempt := 0; attempt < 4; attempt++ {
		req, err = http.NewRequestWithContext(ctx, http.MethodPost, u.String(), nil)
		if err != nil {
			return err
		}
		if err = addAuthentication(req, auth, id); err != nil {
			return err
		}
		response, err = c.Do(req)
		if err != nil {
			return fmt.Errorf("配置已保存，但 LAN 确认请求失败")
		}
		err = status(response)
		response.Body.Close()
		if response.StatusCode == 409 && attempt < 3 {
			timer := time.NewTimer(25 * time.Millisecond)
			select {
			case <-ctx.Done():
				timer.Stop()
				return fmt.Errorf("配置已保存，但 LAN 确认已取消")
			case <-timer.C:
			}
			continue
		}
		if err != nil {
			return fmt.Errorf("配置已保存，但 LAN 确认失败: %w", err)
		}
		return nil
	}
	return fmt.Errorf("配置已保存，但 LAN 确认未完成")
}
