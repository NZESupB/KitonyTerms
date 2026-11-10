package update

import (
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/url"
	"strings"
	"testing"

	"kitonyterms/internal/model"
)

func testBuild() BuildInfo {
	return BuildInfo{Channel: "stable", Version: "0.1.17", Commit: strings.Repeat("a", 40), Build: 10, Attempt: 1, Target: "aarch64-apple-darwin"}
}
func testPackage() Package {
	return Package{Name: "kitonyterms-v0.1.18-macos-aarch64.dmg", Size: 5, SHA256: strings.Repeat("b", 64), Kind: "dmg"}
}
func testManifest() Manifest {
	return Manifest{Schema: 1, Channel: "stable", Version: "0.1.18", Commit: strings.Repeat("b", 40), Build: 11, Attempt: 1, Targets: map[string]targetPackages{"aarch64-apple-darwin": {Archive: testPackage()}}}
}

func TestIdentityChannelsAndPackages(t *testing.T) {
	current := testBuild()
	m := testManifest()
	if newer, err := m.newer(current, "v0.1.18"); err != nil || !newer {
		t.Fatal(newer, err)
	}
	for _, mutate := range []func(*Manifest){func(m *Manifest) { m.Schema = 2 }, func(m *Manifest) { m.Channel = "alpha" }, func(m *Manifest) { m.Commit = "bad" }, func(m *Manifest) { m.Build = 0 }, func(m *Manifest) { m.Version = "01.2.3" }, func(m *Manifest) { m.Version = "1.2.3-alpha" }} {
		m := testManifest()
		mutate(&m)
		if _, err := m.newer(current, "v0.1.18"); err == nil {
			t.Fatal("接受无效身份", m)
		}
	}
	m.Version = "0.1.16"
	if newer, err := m.newer(current, "v0.1.16"); err != nil || newer {
		t.Fatal(newer, err)
	}
	m = testManifest()
	if _, err := m.newer(current, "alpha"); err == nil {
		t.Fatal("接受错误标签")
	}
	current.Channel = "alpha"
	m.Channel = "alpha"
	m.Build = current.Build
	m.Attempt = 2
	if newer, err := m.newer(current, "alpha"); err != nil || !newer {
		t.Fatal(newer, err)
	}
	m.Attempt = 1
	if newer, err := m.newer(current, "alpha"); err != nil || newer {
		t.Fatal(newer, err)
	}
	current.Channel = "development"
	if _, err := m.newer(current, "alpha"); err == nil {
		t.Fatal("开发构建允许更新")
	}
	for _, row := range []struct{ target, kind, suffix string }{{"aarch64-apple-darwin", "dmg", ".dmg"}, {"x86_64-apple-darwin", "dmg", ".dmg"}, {"x86_64-pc-windows-msvc", "zip", ".zip"}, {"aarch64-pc-windows-msvc", "zip", ".zip"}, {"x86_64-unknown-linux-gnu", "tar_gz", ".tar.gz"}, {"aarch64-unknown-linux-gnu", "tar_gz", ".tar.gz"}} {
		p := testPackage()
		p.Kind = row.kind
		p.Name = "kitonyterms-test" + row.suffix
		installer := p
		installer.Kind = "installer"
		installer.Name = "kitonyterms-test-setup.exe"
		m.Targets[row.target] = targetPackages{Archive: p, Installer: &installer}
		if got, err := m.packageFor(row.target, false); err != nil || got != p {
			t.Fatal(row, got, err)
		}
		if row.kind == "zip" {
			if got, err := m.packageFor(row.target, true); err != nil || got != installer {
				t.Fatal(got, err)
			}
		}
	}
	if _, err := m.packageFor("unknown", false); err == nil {
		t.Fatal("接受无发布目标")
	}
	for _, name := range []string{"../kitonyterms-x.dmg", "kitonyterms-a\\b.dmg", "kitonyterms-a:b.dmg", "kitonyterms-..dmg", "kitonyterms-a\x00.dmg"} {
		p := testPackage()
		p.Name = name
		if p.Validate() == nil {
			t.Fatal("接受危险名称")
		}
	}
	p := testPackage()
	p.Size = MaxPackage + 1
	if p.Validate() == nil {
		t.Fatal("接受超限包")
	}
}

func TestJSONAndEmbeddedIdentity(t *testing.T) {
	for _, text := range []string{`{"version":"a","vers\u0069on":"b"}`, `{"targets":{"a":1,"a":2}}`, `{} null`, strings.Repeat("[", 34) + "0" + strings.Repeat("]", 34), string([]byte{0xff})} {
		var value any
		if decodeJSON([]byte(text), &value, false) == nil {
			t.Fatal("接受有歧义JSON")
		}
	}
	before := embeddedIdentity
	t.Cleanup(func() { embeddedIdentity = before })
	embeddedIdentity = ""
	current, err := CurrentBuild()
	if err != nil || current.Channel != "development" {
		t.Fatal(current, err)
	}
	valid := testBuild()
	valid.Target = desktopTarget()
	if valid.Target == "" {
		t.Skip("此平台没有正式更新目标")
	}
	data, _ := json.Marshal(valid)
	embeddedIdentity = base64.RawURLEncoding.EncodeToString(data)
	if got, err := CurrentBuild(); err != nil || got != valid {
		t.Fatal(got, err)
	}
	valid.Target = "invalid"
	data, _ = json.Marshal(valid)
	embeddedIdentity = base64.RawURLEncoding.EncodeToString(data)
	if _, err := CurrentBuild(); err == nil {
		t.Fatal("接受不匹配构建目标")
	}
	embeddedIdentity = "bad!"
	if _, err := CurrentBuild(); err == nil {
		t.Fatal("接受损坏编码")
	}
}

func TestProxyAndRedirectPolicy(t *testing.T) {
	for _, kind := range []string{"system", "direct", "http", "https", "socks5"} {
		p := model.UpdateProxy{Kind: kind}
		if kind != "system" && kind != "direct" {
			p.Host = "::1"
			p.Port = 1080
			p.Username = "测试用户"
		}
		fn, err := proxyFunction(p)
		if err != nil {
			t.Fatal(err)
		}
		if kind == "direct" {
			if fn != nil {
				t.Fatal("直连仍使用代理")
			}
			continue
		}
		if kind == "system" {
			continue
		}
		req, _ := http.NewRequest("GET", officialAPI, nil)
		u, err := fn(req)
		if err != nil || u.User.Username() != p.Username || u.Host != "[::1]:1080" {
			t.Fatal(u, err)
		}
		password, _ := u.User.Password()
		if password != "" {
			t.Fatal("显式代理保存密码")
		}
	}
	for _, p := range []model.UpdateProxy{{Kind: "ftp"}, {Kind: "http", Host: "", Port: 1}, {Kind: "http", Host: "host/path", Port: 1}, {Kind: "http", Host: "host:other", Port: 1}, {Kind: "http", Host: "host", Port: 0}, {Kind: "http", Host: "host", Port: 65536}, {Kind: "http", Host: "host", Port: 1, Username: "x\ny"}, {Kind: "direct", Host: "host"}} {
		if validateProxy(p) == nil {
			t.Fatal("接受无效代理", p.Kind)
		}
	}
	for _, row := range []struct {
		url   string
		valid bool
	}{{"https://api.github.com/x", true}, {"https://release-assets.githubusercontent.com/x?q=1", true}, {"https://github.com:443/x", true}, {"http://github.com/x", false}, {"https://github.com.evil/x", false}, {"https://u@github.com/x", false}, {"https://github.com:444/x", false}, {"https://objects.githubusercontent.com./x", false}} {
		u, _ := url.Parse(row.url)
		if trustedURL(u) != row.valid {
			t.Fatal(row)
		}
	}
	for _, name := range []string{"HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy", "NO_PROXY", "no_proxy"} {
		t.Setenv(name, "")
	}
	t.Setenv("ALL_PROXY", "socks5h://fixture-user:@127.0.0.1:19801")
	fn, err := proxyFunction(model.UpdateProxy{Kind: "system"})
	if err != nil {
		t.Fatal(err)
	}
	req, _ := http.NewRequest("GET", officialAPI, nil)
	proxy, err := fn(req)
	if err != nil || proxy == nil || proxy.Scheme != "socks5h" {
		t.Fatal("System没有沿用ALL_PROXY")
	}
	t.Setenv("NO_PROXY", "api.github.com")
	fn, _ = proxyFunction(model.UpdateProxy{Kind: "system"})
	if got, err := fn(req); err != nil || got != nil {
		t.Fatal("System未遵循NO_PROXY")
	}
}
