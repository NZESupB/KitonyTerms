package syncconfig

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"time"
)

type Endpoint struct {
	URL      string `json:"url"`
	Username string `json:"username"`
	Password string `json:"password"`
}
type RemoteConfig struct {
	Envelope Envelope `json:"envelope"`
	ETag     string   `json:"etag"`
}

func httpURL(value string) (*url.URL, error) {
	u, err := url.Parse(value)
	if err != nil || u.Hostname() == "" || u.User != nil || u.Fragment != "" || (u.Scheme != "http" && u.Scheme != "https") {
		return nil, fmt.Errorf("需要不含凭据或片段的完整 HTTP/HTTPS URL")
	}
	return u, nil
}
func client(lan bool) *http.Client {
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.DialContext = (&net.Dialer{Timeout: 10 * time.Second}).DialContext
	if lan {
		transport.Proxy = nil
	}
	return &http.Client{Transport: transport, Timeout: 60 * time.Second, CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }}
}
func endpointRequest(ctx context.Context, method string, e Endpoint, data []byte) (*http.Request, error) {
	u, err := httpURL(e.URL)
	if err != nil {
		return nil, err
	}
	if u.Scheme != "https" && (e.Username != "" || e.Password != "") {
		return nil, fmt.Errorf("WebDAV 凭据只允许经 HTTPS 发送")
	}
	req, err := http.NewRequestWithContext(ctx, method, u.String(), bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	if e.Username != "" {
		req.SetBasicAuth(e.Username, e.Password)
	}
	return req, nil
}
func status(response *http.Response) error {
	if response.StatusCode == http.StatusPreconditionFailed {
		return fmt.Errorf("ETAG_CONFLICT: 远端配置已改变，请重新下载")
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return fmt.Errorf("同步 HTTP 状态 %d", response.StatusCode)
	}
	return nil
}
func readBounded(response *http.Response, limit int64) ([]byte, error) {
	if response.ContentLength > limit {
		return nil, fmt.Errorf("同步载荷超限")
	}
	data, err := io.ReadAll(io.LimitReader(response.Body, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit {
		return nil, fmt.Errorf("同步载荷超限")
	}
	return data, nil
}
func DownloadWebDAV(ctx context.Context, e Endpoint) (RemoteConfig, error) {
	req, err := endpointRequest(ctx, http.MethodGet, e, nil)
	if err != nil {
		return RemoteConfig{}, err
	}
	c := client(false)
	defer c.CloseIdleConnections()
	response, err := c.Do(req)
	if err != nil {
		return RemoteConfig{}, fmt.Errorf("WebDAV 请求失败")
	}
	defer response.Body.Close()
	if err = status(response); err != nil {
		return RemoteConfig{}, err
	}
	data, err := readBounded(response, MaxEnvelopeBytes)
	if err != nil {
		return RemoteConfig{}, err
	}
	envelope, err := Decode(data)
	if err != nil {
		return RemoteConfig{}, err
	}
	return RemoteConfig{Envelope: envelope, ETag: response.Header.Get("ETag")}, nil
}
func UploadWebDAV(ctx context.Context, e Endpoint, envelope Envelope, etag string, createOnly bool) (string, error) {
	if !createOnly && etag == "" {
		return "", fmt.Errorf("覆盖上传需要远端 ETag；新文件请选择仅创建")
	}
	data, err := envelope.Encode()
	if err != nil {
		return "", err
	}
	req, err := endpointRequest(ctx, http.MethodPut, e, data)
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")
	if createOnly {
		req.Header.Set("If-None-Match", "*")
	} else {
		req.Header.Set("If-Match", etag)
	}
	c := client(false)
	defer c.CloseIdleConnections()
	response, err := c.Do(req)
	if err != nil {
		return "", fmt.Errorf("WebDAV 请求失败")
	}
	defer response.Body.Close()
	if err = status(response); err != nil {
		return "", err
	}
	return response.Header.Get("ETag"), nil
}
