package update

import (
	"context"
	"encoding/json"
	"net/http/httptest"
	"os"
	"strings"
	"sync"
	"testing"
	"time"

	"kitonyterms/internal/model"
)

func waitUpdate(t *testing.T, s *Service, phase string) model.UpdateSnapshot {
	t.Helper()
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		state := s.Snapshot()
		if state.Phase == phase && !state.Busy {
			return state
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("未进入%s，当前%+v", phase, s.Snapshot())
	return model.UpdateSnapshot{}
}
func serviceFeed(t *testing.T, f *feed) (*Service, string) {
	t.Helper()
	server := httptest.NewServer(f)
	t.Cleanup(server.Close)
	path := t.TempDir()
	root, err := openRoot(path)
	if err != nil {
		t.Fatal(err)
	}
	preferences := model.UpdatePreferences{Proxy: model.UpdateProxy{Kind: "direct"}}
	if err = atomicJSON(root, "preferences.json", preferences); err != nil {
		t.Fatal(err)
	}
	root.Close()
	s, err := openService(context.Background(), path, testBuild(), nil, func(p model.UpdateProxy) (*releaseClient, error) {
		c, err := newClient(p)
		if err == nil {
			c.base = server.URL
		}
		return c, err
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := s.Close(); err != nil {
			t.Error(err)
		}
	})
	return s, path
}
func waitRequest(t *testing.T, f *feed, path string) {
	t.Helper()
	timer := time.NewTimer(2 * time.Second)
	defer timer.Stop()
	for {
		select {
		case got := <-f.entered:
			if got == path {
				return
			}
		case <-timer.C:
			t.Fatal("没有启动请求", path)
		}
	}
}
func TestPreferenceCompatibilityAndInvalidProxy(t *testing.T) {
	for _, row := range []struct{ json, kind, notice string }{
		{`{"auto_check":true,"auto_download":false}`, "system", ""},
		{`{"auto_check":true,"auto_download":false,"proxy":{"kind":"http","host":"127.0.0.1","port":3128,"username":null}}`, "http", ""},
		{`{"auto_check":true,"auto_download":false,"proxy":{"kind":"socks5","host":"::1","port":1080,"username":"fixture"}}`, "socks5", ""},
		{`{"auto_check":true,"auto_download":false,"proxy":{"kind":"bad","host":"invalid@fixture"}}`, "system", "invalid_proxy"},
		{`{"auto_check":true,"auto_download":false,"proxy":null}`, "system", "invalid_proxy"},
		{`{"auto_check":true,"auto_download":false,"proxy":{}}`, "system", "invalid_proxy"},
	} {
		root, err := openRoot(t.TempDir())
		if err != nil {
			t.Fatal(err)
		}
		f, err := root.OpenFile("preferences.json", os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
		if err != nil {
			t.Fatal(err)
		}
		f.WriteString(row.json)
		f.Close()
		prefs, notice, err := readPreferences(root, testBuild())
		if err != nil || prefs.Proxy.Kind != row.kind || notice != row.notice || !prefs.AutoCheck || prefs.AutoDownload {
			t.Fatal(prefs, notice, err)
		}
		data, err := readPrivate(root, "preferences.json", 32<<10)
		if err != nil || string(data) != row.json {
			t.Fatal("读取修改了原偏好文件")
		}
		root.Close()
	}
}
func TestDevelopmentAndPreferenceAtomicity(t *testing.T) {
	current := testBuild()
	current.Channel = "development"
	s, err := openService(context.Background(), t.TempDir(), current, nil, newClient)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	before := s.Snapshot()
	if before.Phase != "disabled" || before.Preferences.AutoCheck || before.Preferences.AutoDownload || s.Check() == nil || s.Download() == nil {
		t.Fatal(before)
	}
	request := model.SaveUpdatePreferencesRequest{ExpectedRevision: before.PreferenceRevision, Preferences: before.Preferences}
	request.Preferences.Proxy = model.UpdateProxy{Kind: "direct"}
	if err = s.root.Mkdir("preferences.json", 0700); err != nil {
		t.Fatal(err)
	}
	if _, err = s.SavePreferences(context.Background(), request); err == nil {
		t.Fatal("保存失败被接受")
	}
	if after := s.Snapshot(); after.Preferences != before.Preferences || after.PreferenceRevision != before.PreferenceRevision || after.Phase != "disabled" {
		t.Fatal("失败污染内存")
	}
	s.root.Remove("preferences.json")
	after, err := s.SavePreferences(context.Background(), request)
	if err != nil || after.PreferenceRevision == before.PreferenceRevision || after.Phase != "disabled" {
		t.Fatal(after, err)
	}
	if _, err = s.SavePreferences(context.Background(), request); err == nil {
		t.Fatal("接受旧版本偏好")
	}
	data, err := readPrivate(s.root, "preferences.json", 32<<10)
	if err != nil {
		t.Fatal(err)
	}
	var wire map[string]any
	if err = json.Unmarshal(data, &wire); err != nil || len(wire) != 3 {
		t.Fatal(wire, err)
	}
	if strings.Contains(string(data), "expectedRevision") || strings.Contains(string(data), "password") {
		t.Fatal("偏好文件含事务版本或密码")
	}
}
func TestFailedSaveKeepsTaskAndProxyChangeCancels(t *testing.T) {
	f := newFeed()
	f.mode = "stall"
	s, _ := serviceFeed(t, f)
	if err := s.Check(); err != nil {
		t.Fatal(err)
	}
	waitRequest(t, f, "/releases/latest")
	before := s.Snapshot()
	request := model.SaveUpdatePreferencesRequest{ExpectedRevision: before.PreferenceRevision, Preferences: before.Preferences}
	request.Preferences.Proxy = model.UpdateProxy{Kind: "system"}
	if err := s.root.Remove("preferences.json"); err != nil {
		t.Fatal(err)
	}
	if err := s.root.Mkdir("preferences.json", 0700); err != nil {
		t.Fatal(err)
	}
	if _, err := s.SavePreferences(context.Background(), request); err == nil {
		t.Fatal("接受失败保存")
	}
	if after := s.Snapshot(); after.Phase != "checking" || !after.Busy || after.PreferenceRevision != before.PreferenceRevision || after.Preferences != before.Preferences {
		t.Fatal("失败保存取消了原任务", after)
	}
	s.root.Remove("preferences.json")
	after, err := s.SavePreferences(context.Background(), request)
	if err != nil || after.Busy || after.LastCheck != "" || after.Phase != "idle" {
		t.Fatal(after, err)
	}
	f.mu.Lock()
	f.mode = ""
	f.mu.Unlock()
	if err = s.Check(); err != nil {
		t.Fatal(err)
	}
	state := waitUpdate(t, s, "available")
	state.Available.Notes = "本地副本修改"
	if s.Snapshot().Available.Notes == state.Available.Notes {
		t.Fatal("快照共享可变指针")
	}
	if err = s.Download(); err != nil {
		t.Fatal(err)
	}
	state = waitUpdate(t, s, "downloaded")
	if state.InstallAvailable || state.Received != state.Available.Package.Size || state.Speed != "0" {
		t.Fatal(state)
	}
}
func TestCancelRateLimitAndShutdown(t *testing.T) {
	f := newFeed()
	s, _ := serviceFeed(t, f)
	if err := s.Check(); err != nil {
		t.Fatal(err)
	}
	successful := waitUpdate(t, s, "available")
	f.mu.Lock()
	f.mode = "stall"
	f.mu.Unlock()
	if err := s.Check(); err != nil {
		t.Fatal(err)
	}
	waitRequest(t, f, "/releases/latest")
	if err := s.Check(); err == nil {
		t.Fatal("允许并发检查")
	}
	if err := s.Cancel(); err != nil {
		t.Fatal(err)
	}
	cancelled := s.Snapshot()
	if cancelled.Phase != "cancelled" || cancelled.Busy || cancelled.LastCheck != successful.LastCheck || cancelled.Failure != nil {
		t.Fatal(cancelled)
	}
	if err := s.start("download", true, s.generation-1); err != nil {
		t.Fatal(err)
	}
	if s.Snapshot().Busy {
		t.Fatal("取消后仍启动旧自动下载")
	}
	f.mu.Lock()
	f.mode = "rate"
	f.mu.Unlock()
	if err := s.Check(); err != nil {
		t.Fatal(err)
	}
	limited := waitUpdate(t, s, "error")
	if limited.Failure == nil || limited.Failure.Kind != "rate_limited" || limited.LastCheck != successful.LastCheck {
		t.Fatal(limited)
	}
	f.mu.Lock()
	requests := f.requests["/releases/latest"]
	f.mu.Unlock()
	if err := s.Check(); err == nil {
		t.Fatal("限流时再次请求")
	}
	f.mu.Lock()
	afterRequests := f.requests["/releases/latest"]
	f.mu.Unlock()
	if requests != afterRequests {
		t.Fatal("限流未阻止请求")
	}
	// 清除测试时钟限制，验证关闭会等待下载实际退出。
	s.mu.Lock()
	s.notBefore = time.Time{}
	s.mu.Unlock()
	f.mu.Lock()
	f.mode = "stall-download"
	f.mu.Unlock()
	if err := s.Download(); err != nil {
		t.Fatal(err)
	}
	waitRequest(t, f, "/releases/assets/12")
	var wg sync.WaitGroup
	for range 3 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if err := s.Close(); err != nil {
				t.Error(err)
			}
		}()
	}
	wg.Wait()
	if s.Snapshot().Busy || s.Check() == nil {
		t.Fatal("退出后仍有任务")
	}
}
func TestCacheSymlinksAndJournalPreservation(t *testing.T) {
	root, err := openRoot(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	name := "package-" + strings.Repeat("0", 64)
	if err = atomicJSON(root, name, map[string]int{"fixture": 1}); err != nil {
		t.Fatal(err)
	}
	if err = atomicJSON(root, "journal.json", map[string]int{"schema": 1}); err != nil {
		t.Fatal(err)
	}
	if err = pruneCache(root, "other"); err != nil {
		t.Fatal(err)
	}
	if _, err = root.Stat(name); err != nil {
		t.Fatal("删除了旧事务缓存")
	}
	root.Remove("journal.json")
	if err = atomicJSON(root, "task.json", map[string]int{"fixture": 1}); err != nil {
		t.Fatal(err)
	}
	if err = pruneCache(root, "other"); err != nil {
		t.Fatal(err)
	}
	if _, err = root.Stat(name); err != nil {
		t.Fatal("删除了旧准备任务缓存")
	}
	if err = root.Remove("task.json"); err != nil {
		t.Fatal(err)
	}
	if err = pruneCache(root, "other"); err != nil {
		t.Fatal(err)
	}
	if _, err = root.Stat(name); !os.IsNotExist(err) {
		t.Fatal("未回收旧普通缓存")
	}
	requireUpdateTestSymlink(t, root.Symlink("preferences.json", name))
	if err = verifyCache(context.Background(), root, name, testPackage()); err == nil {
		t.Fatal("接受链接缓存")
	}
}

func TestAvailableReleaseStillChecksWhenDue(t *testing.T) {
	f := newFeed()
	s, _ := serviceFeed(t, f)
	if err := s.Check(); err != nil {
		t.Fatal(err)
	}
	waitUpdate(t, s, "available")
	state := s.Snapshot()
	preferences := state.Preferences
	preferences.AutoCheck = true
	if _, err := s.SavePreferences(context.Background(), model.SaveUpdatePreferencesRequest{ExpectedRevision: state.PreferenceRevision, Preferences: preferences}); err != nil {
		t.Fatal(err)
	}
	for len(f.entered) > 0 {
		<-f.entered
	}
	f.mu.Lock()
	f.mode = "stall"
	f.mu.Unlock()
	s.mu.Lock()
	s.nextCheck = time.Now().Add(-time.Second)
	s.mu.Unlock()
	if err := s.start("check", true, 0); err != nil {
		t.Fatal(err)
	}
	waitRequest(t, f, "/releases/latest")
	if !s.Snapshot().Busy {
		t.Fatal("可用更新永久阻止了到期检查")
	}
	if err := s.Cancel(); err != nil {
		t.Fatal(err)
	}
}

func TestChangingProxyKeepsServerRateLimit(t *testing.T) {
	f := newFeed()
	f.mode = "rate"
	s, _ := serviceFeed(t, f)
	if err := s.Check(); err != nil {
		t.Fatal(err)
	}
	state := waitUpdate(t, s, "error")
	preferences := state.Preferences
	preferences.Proxy = model.UpdateProxy{Kind: "system"}
	if _, err := s.SavePreferences(context.Background(), model.SaveUpdatePreferencesRequest{ExpectedRevision: state.PreferenceRevision, Preferences: preferences}); err != nil {
		t.Fatal(err)
	}
	f.mu.Lock()
	before := f.requests["/releases/latest"]
	f.mu.Unlock()
	if err := s.Check(); err == nil {
		t.Fatal("更换代理绕过了服务器限流")
	}
	state = s.Snapshot()
	if state.Failure == nil || state.Failure.Kind != "rate_limited" || state.Failure.RetryAt == "" {
		t.Fatal("未显示限流与重试时间")
	}
	f.mu.Lock()
	after := f.requests["/releases/latest"]
	f.mu.Unlock()
	if before != after {
		t.Fatal("限流期间发出了请求")
	}
}
