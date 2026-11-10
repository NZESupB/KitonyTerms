//go:build darwin && !ios

package update

import (
	"context"
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func makeTestDMG(t *testing.T, source string) []byte {
	t.Helper()
	image := filepath.Join(t.TempDir(), "fixture.dmg")
	if _, err := runUpdateTool(context.Background(), 30*time.Second, 64<<10, "/usr/bin/hdiutil", "create", "-quiet", "-format", "UDZO", "-volname", "KitonyTermsUpdateTest", "-srcfolder", source, image); err != nil {
		t.Fatal("创建真实DMG夹具", err)
	}
	data, err := os.ReadFile(image)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func makeTestBundle(t *testing.T) string {
	t.Helper()
	source := t.TempDir()
	bundle := filepath.Join(source, "KitonyTerms.app")
	if err := os.MkdirAll(filepath.Join(bundle, "Contents/MacOS"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(bundle, "Contents/Resources"), 0700); err != nil {
		t.Fatal(err)
	}
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	binary, err := os.ReadFile(executable)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(bundle, "Contents/MacOS/kitonyterms"), binary, 0700); err != nil {
		t.Fatal(err)
	}
	plist := `<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.kitonyterms.app</string><key>CFBundleExecutable</key><string>kitonyterms</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleVersion</key><string>123</string></dict></plist>`
	if err = os.WriteFile(filepath.Join(bundle, "Contents/Info.plist"), []byte(plist), 0600); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(bundle, "Contents/Resources/payload.txt"), []byte("原始资源"), 0600); err != nil {
		t.Fatal(err)
	}
	if err = os.Symlink("payload.txt", filepath.Join(bundle, "Contents/Resources/current")); err != nil {
		t.Fatal(err)
	}
	if _, err = runUpdateTool(context.Background(), 20*time.Second, 64<<10, "/usr/bin/codesign", "--force", "--deep", "--sign", "-", bundle); err != nil {
		t.Fatal("临时应用签名", err)
	}
	return source
}

func assertNoPreparationLeftovers(t *testing.T, root *os.Root) {
	t.Helper()
	entries, err := os.ReadDir(root.Name())
	if err != nil {
		t.Fatal(err)
	}
	for _, entry := range entries {
		if strings.HasPrefix(entry.Name(), ".payload-") || entry.Name() == ".dmg-mount" {
			t.Error("失败后遗留暂存或挂载点", entry.Name())
		}
	}
}

func TestDMGPreparationAndFailureCleanup(t *testing.T) {
	t.Setenv("KITONYTERMS_UPDATE_TEST_HELPER", "1")
	root := archiveTestRoot(t)
	source := makeTestBundle(t)
	pkg := cacheTestPackage(t, root, "dmg", makeTestDMG(t, source))
	prepared, err := preparePayload(context.Background(), root, pkg, archiveTestBuild())
	if err != nil {
		t.Fatal("真实DMG准备失败", err)
	}
	if !strings.HasSuffix(prepared.Payload, "/KitonyTerms.app") || !validHex(prepared.SHA256, 64) {
		t.Fatal("准备结果不完整", prepared)
	}
	body, err := root.ReadFile(filepath.ToSlash(filepath.Join(prepared.Directory, "KitonyTerms.app/Contents/Resources/current")))
	if err != nil || string(body) != "原始资源" {
		t.Fatal("相对资源链接未保留", err)
	}
	if _, err = root.Lstat(".dmg-mount"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatal("成功后未卸载", err)
	}
	if err = root.RemoveAll(prepared.Directory); err != nil {
		t.Fatal(err)
	}
	wrong := archiveTestBuild()
	wrong.Build++
	if _, err = preparePayload(context.Background(), root, pkg, wrong); err == nil || !strings.Contains(err.Error(), "构建身份") {
		t.Fatal("构建不匹配未拒绝", err)
	}
	assertNoPreparationLeftovers(t, root)
	if err = os.WriteFile(filepath.Join(source, "KitonyTerms.app/Contents/Resources/payload.txt"), []byte("修改后资源"), 0600); err != nil {
		t.Fatal(err)
	}
	badSignature := cacheTestPackage(t, root, "dmg", makeTestDMG(t, source))
	if _, err = preparePayload(context.Background(), root, badSignature, archiveTestBuild()); err == nil || !strings.Contains(err.Error(), "签名") {
		t.Fatal("签名损坏未拒绝", err)
	}
	assertNoPreparationLeftovers(t, root)
	if err = os.Symlink("../../../outside", filepath.Join(source, "KitonyTerms.app/Contents/Resources/escape")); err != nil {
		t.Fatal(err)
	}
	badLink := cacheTestPackage(t, root, "dmg", makeTestDMG(t, source))
	if _, err = preparePayload(context.Background(), root, badLink, archiveTestBuild()); err == nil || !strings.Contains(err.Error(), "链接越界") {
		t.Fatal("越界链接未拒绝", err)
	}
	assertNoPreparationLeftovers(t, root)
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err = preparePayload(ctx, root, pkg, archiveTestBuild()); !errors.Is(err, context.Canceled) {
		t.Fatal("取消未拒绝", err)
	}
	assertNoPreparationLeftovers(t, root)
}

func TestDMGLinkContainmentAndMountRecovery(t *testing.T) {
	for _, target := range []string{"/tmp/outside", "../../../outside", "..\\outside", "C:bad", "target/"} {
		if err := validateBundleLink("KitonyTerms.app/Contents/Resources/current", target); err == nil {
			t.Fatal("接受危险链接", target)
		}
	}
	if err := validateBundleLink("KitonyTerms.app/Contents/Resources/current", "../Resources/payload"); err != nil {
		t.Fatal(err)
	}
	root := archiveTestRoot(t)
	if err := root.Mkdir(".dmg-mount", 0700); err != nil {
		t.Fatal(err)
	}
	if err := root.Mkdir(".payload-test", 0700); err != nil {
		t.Fatal(err)
	}
	destination, err := root.OpenRoot(".payload-test")
	if err != nil {
		t.Fatal(err)
	}
	defer destination.Close()
	pkg := cacheTestPackage(t, root, "dmg", []byte("无效镜像"))
	file, err := openVerifiedPackage(context.Background(), root, pkg)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	if err = extractDMG(context.Background(), file, newArchiveTree(context.Background(), destination, archiveLimits{MaxExpanded, maxArchiveEntries}), pkg); err == nil {
		t.Fatal("无效镜像未拒绝")
	}
	if _, err = root.Lstat(".dmg-mount"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatal("空旧挂载点未清理", err)
	}
	if _, err = destination.Lstat(".input.dmg"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatal("失败镜像副本未清理", err)
	}
}
