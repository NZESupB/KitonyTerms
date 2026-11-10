//go:build darwin && !ios

package update

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestDesktopDMGPreparationAbortAndDamagedPackageRecovery(t *testing.T) {
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	defer cancel()
	source := makeTestBundle(t)
	image := makeTestDMG(t, source)
	parent := archiveTestRoot(t)
	sourceRoot, err := os.OpenRoot(source)
	if err != nil {
		t.Fatal(err)
	}
	defer sourceRoot.Close()
	if err = copyInstallPayload(ctx, sourceRoot, "KitonyTerms.app", parent, "KitonyTerms.app", true); err != nil {
		t.Fatal(err)
	}
	current := archiveTestBuild()
	current.Version, current.Build = "1.2.2", 122
	identity, err := json.Marshal(current)
	if err != nil {
		t.Fatal(err)
	}
	if err = parent.WriteFile("KitonyTerms.app/Contents/MacOS/.test-build-info.json", identity, 0600); err != nil {
		t.Fatal(err)
	}
	target := filepath.Join(parent.Name(), "KitonyTerms.app")
	if _, err = runUpdateTool(ctx, 20*time.Second, 64<<10, "/usr/bin/codesign", "--force", "--deep", "--sign", "-", target); err != nil {
		t.Fatal(err)
	}
	executable := filepath.Join(target, "Contents/MacOS/kitonyterms")
	d, err := openDesktop(ctx, t.TempDir(), executable, current)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := d.Close(); err != nil {
			t.Error(err)
		}
	})
	s, err := openService(ctx, d.root.Name(), current, d, newClient)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := s.Close(); err != nil {
			t.Error(err)
		}
	})
	pkg := cacheTestPackage(t, d.root, "dmg", image)
	s.mu.Lock()
	s.available = &available{Info: archiveTestBuild(), Package: pkg}
	s.state.Phase = "downloaded"
	s.mu.Unlock()
	if !s.Snapshot().InstallAvailable {
		t.Fatal("正式身份与可写应用包未开放安装")
	}
	if err = s.PrepareInstall(ctx, "dmg-owner"); err != nil {
		t.Fatal("完整准备失败", err)
	}
	state := s.Snapshot()
	if state.Phase != "install_ready" || !state.Busy || state.InstallAvailable {
		t.Fatal("就绪状态错误", state)
	}
	task, err := readInstallTask(d.root)
	if err != nil || !task.Installation.Bundle || task.ParentPID != uint32(os.Getpid()) || task.Current != current || task.Next != archiveTestBuild() {
		t.Fatal("准备任务身份错误", err)
	}
	if err = verifyBuildCommand(ctx, executable, current); err != nil {
		t.Fatal("准备期间改变了旧应用", err)
	}
	assertNoPreparationLeftovers(t, d.root)
	if err = s.AbortInstall("dmg-owner"); err != nil {
		t.Fatal(err)
	}
	if err = pendingInstallation(d.root); err != nil {
		t.Fatal(err)
	}
	if err = requireCleanHelperMarkers(d.root); err != nil {
		t.Fatal(err)
	}
	if err = verifyBundle(ctx, target); err != nil {
		t.Fatal("取消破坏了旧应用签名", err)
	}
	name := "package-" + pkg.SHA256
	if err = d.root.WriteFile(name, []byte("损坏的已下载包"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = s.PrepareInstall(ctx, "damaged-owner"); err == nil {
		t.Fatal("损坏包仍完成准备")
	}
	if err = s.AbortInstall("damaged-owner"); err != nil {
		t.Fatal("归档验证失败后未恢复准备所有权", err)
	}
	state = s.Snapshot()
	if state.Busy || state.Phase != "downloaded" || !state.InstallAvailable || state.Failure == nil || state.Failure.Kind != "install_prepare" {
		t.Fatal("失败后更新服务未恢复", state)
	}
	if err = pendingInstallation(d.root); err != nil {
		t.Fatal("失败后遗留任务", err)
	}
	assertNoPreparationLeftovers(t, d.root)
	if err = verifyBuildCommand(ctx, executable, current); err != nil {
		t.Fatal("失败破坏了旧应用", err)
	}
}
