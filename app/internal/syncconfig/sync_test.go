package syncconfig

import (
	"context"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"kitonyterms/internal/model"
)

func TestEnvelopeRejectsSecretsAndOversize(t *testing.T) {
	data, err := NewEnvelope(model.DefaultConfig()).Encode()
	if err != nil {
		t.Fatal(err)
	}
	if _, err = Decode(data); err != nil {
		t.Fatal(err)
	}
	bad := strings.Replace(string(data), `"config":{`, `"config":{"vault":"private",`, 1)
	if _, err = Decode([]byte(bad)); err == nil {
		t.Fatal("未知机密字段被接受")
	}
	if _, err = Decode(make([]byte, MaxEnvelopeBytes+1)); err == nil {
		t.Fatal("超大配置被接受")
	}
}
func TestWebDAVETagAndTransportBounds(t *testing.T) {
	data, err := NewEnvelope(model.DefaultConfig()).Encode()
	if err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	tag := `"v1"`
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		defer mu.Unlock()
		if r.Method == http.MethodPut {
			if r.Header.Get("If-Match") != tag {
				w.WriteHeader(412)
				return
			}
			body, err := io.ReadAll(r.Body)
			if err != nil {
				t.Error(err)
				return
			}
			if _, err = Decode(body); err != nil {
				t.Error(err)
				return
			}
			tag = `"v2"`
		}
		w.Header().Set("ETag", tag)
		w.Write(data)
	}))
	defer server.Close()
	endpoint := Endpoint{URL: server.URL + "/folder/config.json"}
	remote, err := DownloadWebDAV(context.Background(), endpoint)
	if err != nil || remote.ETag != `"v1"` {
		t.Fatal("ETag 下载失败", err)
	}
	etag, err := UploadWebDAV(context.Background(), endpoint, remote.Envelope, remote.ETag, false)
	if err != nil || etag != `"v2"` {
		t.Fatal("条件写入失败", err)
	}
	if _, err = UploadWebDAV(context.Background(), endpoint, remote.Envelope, remote.ETag, false); err == nil {
		t.Fatal("远端冲突未拒绝")
	}
	endpoint.Username = "user"
	endpoint.Password = "private"
	if _, err = DownloadWebDAV(context.Background(), endpoint); err == nil {
		t.Fatal("HTTP 传输了凭据")
	}
}
func TestLANCommitBeforeAckAndAuthenticationRecovery(t *testing.T) {
	share, err := StartShare(context.Background(), model.DefaultConfig(), "127.0.0.1", time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	defer share.Stop()
	info := share.Info()
	if err = ImportLAN(context.Background(), info.URL, strings.Repeat("0", 26), func(model.Config) error { t.Fatal("错误配对码到达提交"); return nil }); err == nil {
		t.Fatal("错误配对码被接受")
	}
	failed := errors.New("disk failure")
	if err = ImportLAN(context.Background(), info.URL, info.PairingCode, func(model.Config) error { return failed }); !errors.Is(err, failed) {
		t.Fatal("落盘失败未传播", err)
	}
	if share.Finished() {
		t.Fatal("保存失败消费了分享")
	}
	committed := false
	if err = ImportLAN(context.Background(), info.URL, info.PairingCode, func(model.Config) error { committed = true; return nil }); err != nil {
		t.Fatal(err)
	}
	if !committed {
		t.Fatal("未提交配置")
	}
	select {
	case <-share.done:
	case <-time.After(time.Second):
		t.Fatal("ACK 后分享未关闭")
	}
}

func TestLANAckWaitsForDeliveryCompletion(t *testing.T) {
	share, err := StartShare(context.Background(), model.DefaultConfig(), "127.0.0.1", time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	defer share.Stop()
	auth, _, err := keys(share.Info().PairingCode)
	if err != nil {
		t.Fatal(err)
	}
	req, err := http.NewRequest(http.MethodPost, strings.Replace(share.Info().URL, configPath, ackPath, 1), nil)
	if err != nil {
		t.Fatal(err)
	}
	if err = addAuthentication(req, auth, share.id); err != nil {
		t.Fatal(err)
	}
	share.mu.Lock()
	share.state = "delivering"
	share.delivery = make(chan struct{})
	share.mu.Unlock()
	response := httptest.NewRecorder()
	done := make(chan struct{})
	go func() {
		defer close(done)
		share.ServeHTTP(response, req)
	}()
	select {
	case <-done:
		t.Fatal("ACK 未等待交付完成")
	case <-time.After(20 * time.Millisecond):
	}
	share.mu.Lock()
	share.state = "awaiting"
	close(share.delivery)
	share.mu.Unlock()
	select {
	case <-done:
	case <-time.After(time.Second):
		t.Fatal("交付完成后 ACK 未继续")
	}
	if response.Code != http.StatusNoContent {
		t.Fatal("交付完成后 ACK 状态异常", response.Code)
	}
}
func TestLANRejectsNonceReplay(t *testing.T) {
	share, err := StartShare(context.Background(), model.DefaultConfig(), "127.0.0.1", time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	defer share.Stop()
	info := share.Info()
	auth, _, err := keys(info.PairingCode)
	if err != nil {
		t.Fatal(err)
	}
	req, err := http.NewRequest(http.MethodGet, info.URL, nil)
	if err != nil {
		t.Fatal(err)
	}
	if err = addAuthentication(req, auth, nil); err != nil {
		t.Fatal(err)
	}
	c := client(true)
	defer c.CloseIdleConnections()
	response, err := c.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	_, _ = io.Copy(io.Discard, response.Body)
	response.Body.Close()
	if response.StatusCode != 200 {
		t.Fatal("首次认证失败")
	}
	replay := req.Clone(context.Background())
	replay.Header.Set("X-KitonyTerms-Nonce", strings.ToUpper(req.Header.Get("X-KitonyTerms-Nonce")))
	response, err = c.Do(replay)
	if err != nil {
		t.Fatal(err)
	}
	response.Body.Close()
	if response.StatusCode != 409 {
		t.Fatal("nonce 重放被接受", response.StatusCode)
	}
}
