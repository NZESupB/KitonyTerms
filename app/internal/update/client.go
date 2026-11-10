package update

import (
	"context"
	"crypto/sha256"
	"crypto/x509"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"kitonyterms/internal/model"
)

const officialAPI = "https://api.github.com/repos/nzesupb/KitonyTerms"

var errIdle = errors.New("更新响应读取超时")

type failure struct {
	Kind, Stage string
	Status      int
	Retry       time.Duration
}

func (f *failure) Error() string {
	return fmt.Sprintf("更新失败：%s（%s，HTTP %d）", f.Kind, f.Stage, f.Status)
}
func failureOf(err error, stage string) *failure {
	var f *failure
	if errors.As(err, &f) {
		copy := *f
		if copy.Stage == "" {
			copy.Stage = stage
		}
		return &copy
	}
	if errors.Is(err, context.DeadlineExceeded) || errors.Is(err, errIdle) {
		return &failure{Kind: "timeout", Stage: stage}
	}
	var timeout net.Error
	if errors.As(err, &timeout) && timeout.Timeout() {
		return &failure{Kind: "timeout", Stage: stage}
	}
	var unknown x509.UnknownAuthorityError
	var invalid x509.CertificateInvalidError
	var hostname x509.HostnameError
	if errors.As(err, &unknown) || errors.As(err, &invalid) || errors.As(err, &hostname) {
		return &failure{Kind: "tls", Stage: stage}
	}
	// 只用于分类，不向日志、快照或前端返回带代理URL的原始错误。
	text := strings.ToLower(err.Error())
	if strings.Contains(text, "proxy authentication required") || strings.Contains(text, "username/password authentication failed") {
		return &failure{Kind: "proxy_auth", Stage: stage, Status: 407}
	}
	return &failure{Kind: "connection", Stage: stage}
}
func (f *failure) dto(now time.Time) *model.UpdateFailure {
	retry := ""
	if f.Retry > 0 {
		retry = strconv.FormatInt(now.Add(f.Retry).Unix(), 10)
	}
	return &model.UpdateFailure{Kind: f.Kind, Stage: f.Stage, Status: f.Status, RetryAt: retry}
}
func trustedURL(u *url.URL) bool {
	if u.Scheme != "https" || u.User != nil || u.Opaque != "" || u.Port() != "" && u.Port() != "443" {
		return false
	}
	switch strings.ToLower(u.Hostname()) {
	case "api.github.com", "github.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com":
		return true
	}
	return false
}

type releaseClient struct {
	http      *http.Client
	transport *http.Transport
	base      string
	idle      time.Duration
}
type idleBody struct {
	io.ReadCloser
	timer  *time.Timer
	cancel context.CancelCauseFunc
	ctx    context.Context
	idle   time.Duration
}

func (b *idleBody) Read(p []byte) (int, error) {
	n, err := b.ReadCloser.Read(p)
	if n > 0 {
		b.timer.Reset(b.idle)
	}
	if cause := context.Cause(b.ctx); cause != nil {
		return n, cause
	}
	return n, err
}
func (b *idleBody) Close() error {
	b.timer.Stop()
	b.cancel(context.Canceled)
	return b.ReadCloser.Close()
}
func (c *releaseClient) close() { c.transport.CloseIdleConnections() }
func retryDuration(headers http.Header, now time.Time) time.Duration {
	seconds := int64(300)
	if value := headers.Get("Retry-After"); value != "" {
		if number, err := strconv.ParseInt(value, 10, 64); err == nil {
			seconds = number
		} else if date, err := http.ParseTime(value); err == nil {
			seconds = int64(date.Sub(now) / time.Second)
		}
	} else if value := headers.Get("X-RateLimit-Reset"); value != "" {
		if number, err := strconv.ParseInt(value, 10, 64); err == nil {
			seconds = number - now.Unix() + 1
		}
	}
	if seconds < 1 {
		seconds = 1
	}
	if seconds > 86400 {
		seconds = 86400
	}
	return time.Duration(seconds) * time.Second
}
func (c *releaseClient) get(parent context.Context, path, stage string, binary bool) (*http.Response, error) {
	ctx, cancel := context.WithCancelCause(parent)
	timer := time.AfterFunc(c.idle, func() { cancel(errIdle) })
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, c.base+path, nil)
	if err != nil {
		timer.Stop()
		cancel(err)
		return nil, &failure{Kind: "metadata", Stage: stage}
	}
	accept := "application/vnd.github+json"
	if binary {
		accept = "application/octet-stream"
	}
	request.Header.Set("Accept", accept)
	request.Header.Set("User-Agent", "KitonyTerms-updater/1")
	request.Header.Set("X-GitHub-Api-Version", "2022-11-28")
	response, err := c.http.Do(request)
	if err != nil {
		timer.Stop()
		if cause := context.Cause(ctx); cause != nil {
			err = cause
		}
		cancel(context.Canceled)
		return nil, failureOf(err, stage)
	}
	response.Body = &idleBody{ReadCloser: response.Body, timer: timer, cancel: cancel, ctx: ctx, idle: c.idle}
	timer.Reset(c.idle)
	if response.StatusCode != http.StatusOK {
		response.Body.Close()
		f := &failure{Kind: "http_status", Stage: stage, Status: response.StatusCode}
		if response.StatusCode == 403 || response.StatusCode == 429 {
			f.Kind = "rate_limited"
			f.Retry = retryDuration(response.Header, time.Now())
		}
		if response.StatusCode == 407 {
			f.Kind = "proxy_auth"
		}
		return nil, f
	}
	if encoding := response.Header.Get("Content-Encoding"); encoding != "" && encoding != "identity" {
		response.Body.Close()
		return nil, &failure{Kind: "metadata", Stage: stage}
	}
	return response, nil
}
func (c *releaseClient) asset(ctx context.Context, id uint64, stage string) (*http.Response, error) {
	if id == 0 {
		return nil, &failure{Kind: "metadata", Stage: stage}
	}
	return c.get(ctx, "/releases/assets/"+strconv.FormatUint(id, 10), stage, true)
}
func bounded(response *http.Response, limit int, stage string) ([]byte, error) {
	defer response.Body.Close()
	if response.ContentLength > int64(limit) {
		return nil, &failure{Kind: "size", Stage: stage}
	}
	data, err := io.ReadAll(io.LimitReader(response.Body, int64(limit)+1))
	if err != nil {
		return nil, failureOf(err, stage)
	}
	if len(data) > limit {
		return nil, &failure{Kind: "size", Stage: stage}
	}
	return data, nil
}

type releaseAsset struct {
	ID     uint64  `json:"id"`
	Name   string  `json:"name"`
	Size   uint64  `json:"size"`
	Digest *string `json:"digest"`
}
type release struct {
	ID          uint64         `json:"id"`
	Tag         string         `json:"tag_name"`
	Draft       *bool          `json:"draft"`
	Prerelease  *bool          `json:"prerelease"`
	Assets      []releaseAsset `json:"assets"`
	Body        string         `json:"body"`
	PublishedAt string         `json:"published_at"`
}

func uniqueAsset(assets []releaseAsset, name string) (releaseAsset, error) {
	var found releaseAsset
	for _, asset := range assets {
		if asset.Name == name {
			if found.ID != 0 || asset.ID == 0 {
				return found, &failure{Kind: "metadata"}
			}
			found = asset
		}
	}
	if found.ID == 0 {
		return found, &failure{Kind: "metadata"}
	}
	return found, nil
}
func verifyDigest(data []byte, expected string) bool {
	hash := sha256.Sum256(data)
	return expected == "sha256:"+hex.EncodeToString(hash[:])
}
func (c *releaseClient) check(ctx context.Context, current BuildInfo, installed bool) (*available, error) {
	if err := current.Validate(); err != nil {
		return nil, &failure{Kind: "metadata", Stage: "release"}
	}
	endpoint := "/releases/latest"
	if current.Channel == "alpha" {
		endpoint = "/releases/tags/alpha"
	}
	response, err := c.get(ctx, endpoint, "release", false)
	if err != nil {
		return nil, err
	}
	data, err := bounded(response, metadataLimit, "release")
	if err != nil {
		return nil, err
	}
	var r release
	if err = decodeJSON(data, &r, false); err != nil || r.ID == 0 || r.Draft == nil || r.Prerelease == nil || *r.Draft || *r.Prerelease != (current.Channel == "alpha") || len(r.Assets) > 4096 {
		return nil, &failure{Kind: "metadata", Stage: "release"}
	}
	seen := make(map[uint64]bool)
	for _, asset := range r.Assets {
		if asset.ID == 0 || seen[asset.ID] || len(asset.Name) > 512 {
			return nil, &failure{Kind: "metadata", Stage: "release"}
		}
		seen[asset.ID] = true
	}
	asset, err := uniqueAsset(r.Assets, "update-manifest.json")
	if err != nil {
		return nil, &failure{Kind: "metadata", Stage: "manifest"}
	}
	if asset.Size == 0 || asset.Size > metadataLimit {
		return nil, &failure{Kind: "size", Stage: "manifest"}
	}
	response, err = c.asset(ctx, asset.ID, "manifest")
	if err != nil {
		return nil, err
	}
	data, err = bounded(response, metadataLimit, "manifest")
	if err != nil {
		return nil, err
	}
	if uint64(len(data)) != asset.Size {
		return nil, &failure{Kind: "size", Stage: "manifest"}
	}
	if asset.Digest != nil && !verifyDigest(data, *asset.Digest) {
		return nil, &failure{Kind: "digest", Stage: "manifest"}
	}
	var manifest Manifest
	if err = decodeJSON(data, &manifest, true); err != nil {
		return nil, &failure{Kind: "metadata", Stage: "manifest"}
	}
	newer, err := manifest.newer(current, r.Tag)
	if err != nil {
		return nil, &failure{Kind: "metadata", Stage: "manifest"}
	}
	if !newer {
		return nil, nil
	}
	pkg, err := manifest.packageFor(current.Target, installed)
	if err != nil {
		return nil, &failure{Kind: "metadata", Stage: "manifest"}
	}
	asset, err = uniqueAsset(r.Assets, pkg.Name)
	if err != nil {
		return nil, &failure{Kind: "metadata", Stage: "release"}
	}
	if asset.Size != pkg.Size {
		return nil, &failure{Kind: "size", Stage: "release"}
	}
	if asset.Digest != nil && *asset.Digest != "sha256:"+pkg.SHA256 {
		return nil, &failure{Kind: "digest", Stage: "release"}
	}
	if len(r.PublishedAt) > 64 {
		return nil, &failure{Kind: "metadata", Stage: "release"}
	}
	if r.PublishedAt != "" {
		if _, err = time.Parse(time.RFC3339, r.PublishedAt); err != nil {
			return nil, &failure{Kind: "metadata", Stage: "release"}
		}
	}
	truncated := len(r.Body) > notesLimit
	if truncated {
		end := notesLimit
		for !utf8.RuneStart(r.Body[end]) {
			end--
		}
		r.Body = r.Body[:end]
	}
	return &available{Info: BuildInfo{Channel: manifest.Channel, Version: manifest.Version, Commit: manifest.Commit, Build: manifest.Build, Attempt: manifest.Attempt, Target: current.Target}, Package: pkg, AssetID: asset.ID, ReleaseID: r.ID, Tag: r.Tag, Notes: r.Body, NotesTruncated: truncated, PublishedAt: r.PublishedAt}, nil
}
