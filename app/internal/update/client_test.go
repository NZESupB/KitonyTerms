package update

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf8"

	"kitonyterms/internal/model"
)

type feed struct {
	mu             sync.Mutex
	manifest       Manifest
	releaseMutate  func(*release)
	manifestMutate func([]byte) []byte
	payload        []byte
	mode           string
	requests       map[string]int
	entered        chan string
}

func newFeed() *feed {
	payload := bytes.Repeat([]byte("更新测试数据\n"), 4096)
	hash := sha256.Sum256(payload)
	m := testManifest()
	entry := m.Targets[testBuild().Target]
	entry.Archive.Size = uint64(len(payload))
	entry.Archive.SHA256 = hex.EncodeToString(hash[:])
	m.Targets[testBuild().Target] = entry
	return &feed{manifest: m, payload: payload, requests: make(map[string]int), entered: make(chan string, 32)}
}
func (f *feed) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	f.requests[r.URL.Path]++
	mode := f.mode
	payload := bytes.Clone(f.payload)
	manifest := f.manifest
	data, _ := json.Marshal(manifest)
	if f.manifestMutate != nil {
		data = f.manifestMutate(data)
	}
	no := false
	p := manifest.Targets[testBuild().Target].Archive
	manifestDigest := "sha256:" + fmt.Sprintf("%x", sha256.Sum256(data))
	packageDigest := "sha256:" + p.SHA256
	rel := release{ID: 5, Tag: "v" + manifest.Version, Draft: &no, Prerelease: &no, Assets: []releaseAsset{{ID: 11, Name: "update-manifest.json", Size: uint64(len(data)), Digest: &manifestDigest}, {ID: 12, Name: p.Name, Size: p.Size, Digest: &packageDigest}}, Body: strings.Repeat("中文", 12000) + "<img src=x onerror=alert(1)>", PublishedAt: "2026-11-02T00:00:00Z"}
	if f.releaseMutate != nil {
		f.releaseMutate(&rel)
	}
	f.mu.Unlock()
	select {
	case f.entered <- r.URL.Path:
	default:
	}
	if mode == "stall" || mode == "stall-download" && r.URL.Path == "/releases/assets/12" {
		w.WriteHeader(200)
		w.(http.Flusher).Flush()
		<-r.Context().Done()
		return
	}
	if mode == "rate" {
		w.Header().Set("Retry-After", "120")
		w.WriteHeader(429)
		return
	}
	if r.Header.Get("Accept") == "" || r.Header.Get("User-Agent") != "KitonyTerms-updater/1" || r.Header.Get("X-GitHub-Api-Version") != "2022-11-28" {
		w.WriteHeader(400)
		return
	}
	switch r.URL.Path {
	case "/releases/latest", "/releases/tags/alpha":
		json.NewEncoder(w).Encode(rel)
	case "/releases/assets/11":
		w.Write(data)
	case "/releases/assets/12":
		if mode == "bad-digest" {
			payload[0] ^= 1
		}
		if mode == "truncated" {
			payload = payload[:len(payload)-1]
		}
		if mode == "oversize" {
			payload = append(payload, 'x')
			w.(http.Flusher).Flush()
		}
		w.Write(payload)
	default:
		w.WriteHeader(404)
	}
}
func testClient(t *testing.T, handler http.Handler) (*releaseClient, *httptest.Server) {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	client, err := newClient(model.UpdateProxy{Kind: "direct"})
	if err != nil {
		t.Fatal(err)
	}
	client.base = server.URL
	t.Cleanup(client.close)
	return client, server
}
func requireFailure(t *testing.T, err error, kind string) {
	t.Helper()
	var f *failure
	if !errors.As(err, &f) || f.Kind != kind {
		t.Fatalf("期望%s，实际%v", kind, err)
	}
}

func TestReleaseAssetBindingAndNotes(t *testing.T) {
	f := newFeed()
	client, _ := testClient(t, f)
	got, err := client.check(context.Background(), testBuild(), false)
	if err != nil || got == nil || got.AssetID != 12 || got.ReleaseID != 5 || !got.NotesTruncated || !utf8.ValidString(got.Notes) || len(got.Notes) > notesLimit {
		t.Fatal(got, err)
	}
	for _, row := range []struct {
		name, kind string
		mutate     func(*release)
	}{
		{"missing-draft", "metadata", func(r *release) { r.Draft = nil }},
		{"draft", "metadata", func(r *release) { yes := true; r.Draft = &yes }},
		{"wrong-channel", "metadata", func(r *release) { yes := true; r.Prerelease = &yes }},
		{"duplicate-name", "metadata", func(r *release) { a := r.Assets[0]; a.ID = 19; r.Assets = append(r.Assets, a) }},
		{"duplicate-id", "metadata", func(r *release) { r.Assets[1].ID = r.Assets[0].ID }},
		{"tag", "metadata", func(r *release) { r.Tag = "v99.0.0" }},
		{"manifest-digest", "digest", func(r *release) { bad := "sha256:" + strings.Repeat("0", 64); r.Assets[0].Digest = &bad }},
		{"manifest-length", "size", func(r *release) { r.Assets[0].Size++ }},
		{"asset-digest", "digest", func(r *release) { bad := "sha256:" + strings.Repeat("0", 64); r.Assets[1].Digest = &bad }},
		{"asset-size", "size", func(r *release) { r.Assets[1].Size++ }},
	} {
		t.Run(row.name, func(t *testing.T) {
			f.mu.Lock()
			f.releaseMutate = row.mutate
			f.mu.Unlock()
			_, err := client.check(context.Background(), testBuild(), false)
			requireFailure(t, err, row.kind)
		})
	}
	f.mu.Lock()
	f.releaseMutate = nil
	f.manifestMutate = func(data []byte) []byte {
		return bytes.Replace(data, []byte(`"schema":1`), []byte(`"schema":1,"schema":1`), 1)
	}
	f.mu.Unlock()
	_, err = client.check(context.Background(), testBuild(), false)
	requireFailure(t, err, "metadata")
}

func TestHTTPBoundsRedirectAndIdleTimeout(t *testing.T) {
	for _, row := range []struct {
		status int
		kind   string
	}{{403, "rate_limited"}, {429, "rate_limited"}, {407, "proxy_auth"}, {503, "http_status"}} {
		t.Run(fmt.Sprint(row.status), func(t *testing.T) {
			client, _ := testClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("Retry-After", "999999999")
				w.WriteHeader(row.status)
			}))
			_, err := client.check(context.Background(), testBuild(), false)
			requireFailure(t, err, row.kind)
			if f := failureOf(err, "release"); f.Retry > 24*time.Hour {
				t.Fatal("重试时间无界")
			}
		})
	}
	t.Run("bounded", func(t *testing.T) {
		client, _ := testClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Length", fmt.Sprint(metadataLimit+1))
			w.WriteHeader(200)
		}))
		_, err := client.check(context.Background(), testBuild(), false)
		requireFailure(t, err, "size")
	})
	t.Run("redirect", func(t *testing.T) {
		var reached bool
		other := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { reached = true }))
		defer other.Close()
		client, _ := testClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { http.Redirect(w, r, other.URL, 302) }))
		_, err := client.check(context.Background(), testBuild(), false)
		requireFailure(t, err, "redirect")
		if reached {
			t.Fatal("跟随了不可信跳转")
		}
	})
	t.Run("idle", func(t *testing.T) {
		client, _ := testClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(200)
			w.(http.Flusher).Flush()
			<-r.Context().Done()
		}))
		client.idle = 40 * time.Millisecond
		_, err := client.check(context.Background(), testBuild(), false)
		requireFailure(t, err, "timeout")
	})
}

func TestStreamDownloadChecksumCacheAndCancellation(t *testing.T) {
	f := newFeed()
	client, _ := testClient(t, f)
	ctx := context.Background()
	candidate, err := client.check(ctx, testBuild(), false)
	if err != nil {
		t.Fatal(err)
	}
	root, err := openRoot(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	var finalReceived, finalSpeed uint64
	progress := func(received, speed uint64) { finalReceived = received; finalSpeed = speed }
	if err = client.download(ctx, root, *candidate, progress); err != nil {
		t.Fatal(err)
	}
	if finalReceived != candidate.Package.Size || finalSpeed != 0 {
		t.Fatal("完成进度错误")
	}
	f.mu.Lock()
	first := f.requests["/releases/assets/12"]
	f.mu.Unlock()
	if err = client.download(ctx, root, *candidate, progress); err != nil {
		t.Fatal(err)
	}
	f.mu.Lock()
	second := f.requests["/releases/assets/12"]
	f.mu.Unlock()
	if first != second {
		t.Fatal("有效缓存仍下载")
	}
	cached := "package-" + candidate.Package.SHA256
	if err = atomicJSON(root, cached, map[string]string{"broken": "cache"}); err != nil {
		t.Fatal(err)
	}
	if err = client.download(ctx, root, *candidate, progress); err != nil {
		t.Fatal(err)
	}
	if err = verifyCache(ctx, root, cached, candidate.Package); err != nil {
		t.Fatal(err)
	}
	for _, row := range []struct{ mode, kind string }{{"bad-digest", "digest"}, {"truncated", "size"}, {"oversize", "size"}} {
		t.Run(row.mode, func(t *testing.T) {
			root.Remove(cached)
			f.mu.Lock()
			f.mode = row.mode
			f.mu.Unlock()
			err := client.download(ctx, root, *candidate, progress)
			requireFailure(t, err, row.kind)
			if _, err := root.Stat(cached); err == nil {
				t.Fatal("错误数据进入缓存")
			}
		})
	}
	f.mu.Lock()
	f.mode = "stall-download"
	f.mu.Unlock()
	for len(f.entered) > 0 {
		<-f.entered
	}
	request, cancel := context.WithCancel(ctx)
	defer cancel()
	done := make(chan error, 1)
	go func() { done <- client.download(request, root, *candidate, func(uint64, uint64) {}) }()
	deadline := time.After(2 * time.Second)
	for {
		select {
		case path := <-f.entered:
			if path == "/releases/assets/12" {
				cancel()
				goto cancelled
			}
		case <-deadline:
			t.Fatal("下载未启动")
		}
	}
cancelled:
	select {
	case err := <-done:
		if err == nil {
			t.Fatal("取消下载成功")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("取消未收敛")
	}
	entries, err := root.Open(".")
	if err != nil {
		t.Fatal(err)
	}
	defer entries.Close()
	names, _ := entries.Readdirnames(-1)
	for _, name := range names {
		if strings.HasPrefix(name, ".update-") {
			t.Fatal("取消留下临时文件")
		}
	}
}
