package update

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"kitonyterms/internal/model"
)

func installationExecutable(t *testing.T, bundle bool, name string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "中文 空格", name)
	if bundle {
		path = filepath.Join(filepath.Dir(path), "KitonyTerms.app", "Contents", "MacOS", "kitonyterms")
	}
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("仅用于路径发现的普通文件\n"), 0700); err != nil {
		t.Fatal(err)
	}
	canonical, err := canonicalUpdatePath(path)
	if err != nil {
		t.Fatal(err)
	}
	return canonical
}

func TestInstallationDiscoveryCanonicalModes(t *testing.T) {
	for _, platform := range []string{"darwin", "windows", "linux"} {
		t.Run(platform, func(t *testing.T) {
			executable := installationExecutable(t, platform == "darwin", "kitonyterms.exe")
			calls := 0
			install, err := inspectInstallation(executable, platform, func(string) (bool, error) { calls++; return true, nil })
			if err != nil || install.Executable != executable || install.Bundle != (platform == "darwin") || install.Installed || calls != 0 {
				t.Fatal(install, calls, err)
			}
			if platform == "darwin" && install.Target != filepath.Dir(filepath.Dir(filepath.Dir(executable))) {
				t.Fatal("未发现完整应用包", install)
			}
			if platform != "darwin" && install.Target != executable {
				t.Fatal("便携目标发生变化", install)
			}
		})
	}
	for _, path := range []string{"relative/kitonyterms", t.TempDir(), filepath.Join(t.TempDir(), "missing")} {
		if _, err := inspectInstallation(path, "linux", nil); err == nil {
			t.Fatal("接受无效应用路径", path)
		}
	}
}

func TestInstallationRegistrationRequiresFilesAndTrustedLookup(t *testing.T) {
	for _, mode := range []string{"portable", "marker_only", "uninstaller_only", "registered", "foreign", "registry_error", "marker_directory"} {
		t.Run(mode, func(t *testing.T) {
			executable := installationExecutable(t, false, "kitonyterms.exe")
			parent := filepath.Dir(executable)
			for _, name := range []string{"kitonyterms-installation.txt", "unins000.exe"} {
				if mode == "portable" || mode == "marker_only" && name == "unins000.exe" || mode == "uninstaller_only" && name == "kitonyterms-installation.txt" {
					continue
				}
				path := filepath.Join(parent, name)
				var err error
				if mode == "marker_directory" && name == "kitonyterms-installation.txt" {
					err = os.Mkdir(path, 0700)
				} else {
					err = os.WriteFile(path, []byte("隔离登记夹具"), 0600)
				}
				if err != nil {
					t.Fatal(err)
				}
			}
			calls := 0
			failure := fmt.Errorf("受控登记读取失败：%w", fs.ErrPermission)
			install, err := inspectInstallation(executable, "windows", func(got string) (bool, error) {
				calls++
				if got != executable {
					t.Fatal("登记查询未使用规范程序路径", got)
				}
				if mode == "registry_error" {
					return false, failure
				}
				return mode != "foreign", nil
			})
			if mode == "registry_error" {
				if !errors.Is(err, failure) {
					t.Fatal("登记失败降级成便携版", err)
				}
				return
			}
			if mode == "marker_directory" {
				if err == nil || calls != 0 {
					t.Fatal("无效登记文件仍查询注册表", calls, err)
				}
				return
			}
			if err != nil || install.Installed != (mode == "registered") {
				t.Fatal(install, err)
			}
			expected := 0
			if mode == "registered" || mode == "foreign" {
				expected = 1
			}
			if calls != expected {
				t.Fatal("登记查询条件不匹配", calls)
			}
		})
	}
}

func TestInstallationAccessRejectsUnsafeLocationsAndCleansProbe(t *testing.T) {
	updates := archiveTestRoot(t)
	executable := installationExecutable(t, false, "kitonyterms")
	portable := installationAt(executable, "linux")
	if access := assessInstallation(context.Background(), updates, portable, "linux"); access.Reason != "" || access.Elevate || access.Err != nil {
		t.Fatal(access)
	}
	entries, err := os.ReadDir(filepath.Dir(executable))
	if err != nil || len(entries) != 1 {
		t.Fatal("目录探测留下文件", entries, err)
	}
	bundle := installationAt(installationExecutable(t, true, "kitonyterms"), "darwin")
	for _, row := range []struct {
		install installation
		reason  string
	}{
		{portable, "not_app_bundle"},
		{installation{Target: "/Volumes/KitonyTerms.app", Bundle: true}, "mounted_installation"},
		{installation{Target: "/private/tmp/AppTranslocation/fixture/KitonyTerms.app", Bundle: true}, "translocated_installation"},
		{bundle, ""},
	} {
		access := assessInstallation(context.Background(), updates, row.install, "darwin")
		if access.Reason != row.reason || access.Elevate || (access.Err != nil) != (row.reason != "") {
			t.Fatal(row.reason, access)
		}
	}
	inside, err := os.OpenRoot(bundle.Target)
	if err != nil {
		t.Fatal(err)
	}
	defer inside.Close()
	if access := assessInstallation(context.Background(), inside, bundle, "darwin"); access.Reason != "installation_unavailable" || access.Err == nil {
		t.Fatal("允许事务位于应用包内", access)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	parent, err := os.OpenRoot(filepath.Dir(executable))
	if err != nil {
		t.Fatal(err)
	}
	defer parent.Close()
	if created, err := probeInstallationDirectory(ctx, parent); created || !errors.Is(err, context.Canceled) {
		t.Fatal("取消的探测仍创建文件", created, err)
	}
	if err := os.Chmod(executable, 0500); err != nil {
		t.Fatal(err)
	}
	defer os.Chmod(executable, 0700)
	if access := assessInstallation(context.Background(), updates, portable, "windows"); access.Reason != "readonly_installation" || access.Elevate {
		t.Fatal("只读目标仍请求授权", access)
	}
	if runtime.GOOS != "windows" {
		if access := assessInstallation(context.Background(), updates, portable, "linux"); access.Reason != "" {
			t.Fatal("将Unix目标只读误判为目录不可替换", access)
		}
	}
}

func TestInstallationPermissionProbeOnlyElevatesWindowsCreationFailure(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("目录ACL需Windows原生专用夹具，此项使用Unix权限制造真实拒绝")
	}
	updates := archiveTestRoot(t)
	executable := installationExecutable(t, false, "kitonyterms")
	parent := filepath.Dir(executable)
	if err := os.Chmod(parent, 0500); err != nil {
		t.Fatal(err)
	}
	defer os.Chmod(parent, 0700)
	install := installationAt(executable, "linux")
	access := assessInstallation(context.Background(), updates, install, "linux")
	if access.Reason == "" {
		t.Skip("当前进程可绕过目录写权限，不能作为拒绝证据")
	}
	if access.Reason != "installation_unwritable" || !errors.Is(access.Err, fs.ErrPermission) || access.Elevate {
		t.Fatal(access)
	}
	access = assessInstallation(context.Background(), updates, install, "windows")
	if access.Reason != "" || access.Err != nil || !access.Elevate {
		t.Fatal("Windows目录权限拒绝未进入受限授权", access)
	}
	install.Installed = true
	access = assessInstallation(context.Background(), updates, install, "windows")
	if access.Reason != "" || access.Elevate || access.Err != nil {
		t.Fatal("注册安装错误探测父目录", access)
	}
	entries, err := os.ReadDir(parent)
	if err != nil || len(entries) != 1 {
		t.Fatal("权限探测留下文件", entries, err)
	}
}

func desktopDiscoveryFixture(t *testing.T, current BuildInfo) *Desktop {
	t.Helper()
	executable := installationExecutable(t, runtime.GOOS == "darwin", "kitonyterms.exe")
	desktop, err := openDesktop(context.Background(), t.TempDir(), executable, current)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := desktop.Close(); err != nil {
			t.Error(err)
		}
	})
	return desktop
}

func TestInstallationServiceRequiresMatchingOwnerAndKeepsDisabledDevelopment(t *testing.T) {
	for _, mode := range []string{"normal", "foreign_root", "foreign_build", "closed", "recovery", "development"} {
		t.Run(mode, func(t *testing.T) {
			current := archiveTestBuild()
			if mode == "development" {
				current.Channel = "development"
			}
			desktop := desktopDiscoveryFixture(t, current)
			path := desktop.root.Name()
			switch mode {
			case "foreign_root":
				path = t.TempDir()
			case "foreign_build":
				current.Build++
			case "closed":
				if err := desktop.Close(); err != nil {
					t.Fatal(err)
				}
			case "recovery":
				desktop.recovery = &installJournal{}
			case "development":
				// 删除当前文件仍可打开禁用服务，证明开发构建没有再次发现/探测目标。
				if err := os.Remove(desktop.install.Executable); err != nil {
					t.Fatal(err)
				}
			}
			s, err := openService(context.Background(), path, current, desktop, newClient)
			if mode != "normal" && mode != "development" {
				if err == nil || s != nil {
					t.Fatal("服务接受了错误桌面所有者", s, err)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			defer s.Close()
			state := s.Snapshot()
			if state.InstallAvailable || state.RequiresElevation {
				t.Fatal("发现位置就启用产品安装", state)
			}
			if mode == "development" && (state.Phase != "disabled" || state.DisabledReason != "development" || s.Check() == nil) {
				t.Fatal(state)
			}
			if mode == "normal" && (state.Phase != "idle" || s.installation.Executable != desktop.install.Executable) {
				t.Fatal(state)
			}
		})
	}
	if s, err := New(context.Background(), t.TempDir(), nil); err == nil || s != nil {
		t.Fatal("公共构造允许缺少进程锁", s, err)
	}
}

func TestInstallationServiceSelectsRegisteredInstallerAsset(t *testing.T) {
	for _, installed := range []bool{false, true} {
		t.Run(fmt.Sprint(installed), func(t *testing.T) {
			feed := newFeed()
			payloads := map[string][]byte{"/releases/assets/13": []byte("隔离安装器下载数据"), "/releases/assets/14": []byte("隔离便携归档下载数据")}
			packageOf := func(name, kind, path string) Package {
				h := sha256.Sum256(payloads[path])
				return Package{Name: name, Kind: kind, Size: uint64(len(payloads[path])), SHA256: hex.EncodeToString(h[:])}
			}
			setup := packageOf("kitonyterms-v0.1.18-windows-x86_64-setup.exe", "installer", "/releases/assets/13")
			archive := packageOf("kitonyterms-v0.1.18-windows-x86_64.zip", "zip", "/releases/assets/14")
			current := testBuild()
			current.Target = "x86_64-pc-windows-msvc"
			feed.manifest.Targets[current.Target] = targetPackages{Archive: archive, Installer: &setup}
			feed.releaseMutate = func(release *release) {
				for index, pkg := range []Package{setup, archive} {
					sha := "sha256:" + pkg.SHA256
					release.Assets = append(release.Assets, releaseAsset{ID: uint64(13 + index), Name: pkg.Name, Size: pkg.Size, Digest: &sha})
				}
			}
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if payload, ok := payloads[r.URL.Path]; ok {
					w.Write(payload)
					return
				}
				feed.ServeHTTP(w, r)
			}))
			defer server.Close()
			path := t.TempDir()
			root, err := openRoot(path)
			if err != nil {
				t.Fatal(err)
			}
			if err = atomicJSON(root, "preferences.json", model.UpdatePreferences{Proxy: model.UpdateProxy{Kind: "direct"}}); err != nil {
				t.Fatal(err)
			}
			root.Close()
			s, err := openService(context.Background(), path, current, nil, func(proxy model.UpdateProxy) (*releaseClient, error) {
				client, err := newClient(proxy)
				if err == nil {
					client.base = server.URL
				}
				return client, err
			})
			if err != nil {
				t.Fatal(err)
			}
			defer s.Close()
			// 此项只验证服务到真实HTTP资产的选择；登记发现已由独立夹具覆盖。
			s.installation.Installed = installed
			if err = s.Check(); err != nil {
				t.Fatal(err)
			}
			state := waitUpdate(t, s, "available")
			want := archive
			wantID := "14"
			if installed {
				want = setup
				wantID = "13"
			}
			if state.Available.Package.Kind != want.Kind || state.Available.AssetID != wantID {
				t.Fatal("选择了错误安装包", state)
			}
			if err = s.Download(); err != nil {
				t.Fatal(err)
			}
			state = waitUpdate(t, s, "downloaded")
			data, err := s.root.ReadFile("package-" + want.SHA256)
			if err != nil || string(data) != string(payloads["/releases/assets/"+wantID]) || state.InstallAvailable {
				t.Fatal("未下载所选资产或错误启用安装", err, state)
			}
		})
	}
}
