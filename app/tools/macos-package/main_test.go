package main

import (
	"bytes"
	"context"
	"encoding/json"
	"encoding/xml"
	"errors"
	"io"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"kitonyterms/internal/update"
)

func testIdentity() update.BuildInfo {
	target := "aarch64-apple-darwin"
	if runtime.GOOS == "darwin" && runtime.GOARCH == "amd64" {
		target = "x86_64-apple-darwin"
	}
	return update.BuildInfo{Channel: "stable", Version: "0.1.18", Commit: strings.Repeat("e", 40), Build: 604, Attempt: 1, Target: target}
}

func TestMain(m *testing.M) {
	if os.Getenv("KITONYTERMS_MACOS_PACKAGE_TEST_HELPER") == "1" {
		switch os.Getenv("KITONYTERMS_MACOS_PACKAGE_TEST_MODE") {
		case "stdout":
			os.Stdout.Write(bytes.Repeat([]byte("x"), 1<<20))
		case "stderr":
			os.Stderr.Write(bytes.Repeat([]byte("x"), 1<<20))
		case "sleep":
			time.Sleep(time.Minute)
		case "create":
			os.MkdirAll(os.Getenv("KITONYTERMS_DATA_DIR"), 0700)
			fallthrough
		case "identity":
			json.NewEncoder(os.Stdout).Encode(testIdentity())
		}
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func TestIdentityAndPlistFidelity(t *testing.T) {
	info := testIdentity()
	info.Build, info.Attempt = math.MaxUint64, math.MaxUint64
	encoded, _ := json.Marshal(info)
	actual, err := parseIdentity(encoded)
	if err != nil || actual != info {
		t.Fatal("完整身份不能截断", err)
	}
	plist, err := candidatePlist(info)
	if err != nil {
		t.Fatal(err)
	}
	decoder := xml.NewDecoder(bytes.NewReader(plist))
	var key string
	values := map[string]string{}
	booleans := map[string]bool{}
	for {
		token, err := decoder.Token()
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatal(err)
		}
		if start, ok := token.(xml.StartElement); ok {
			switch start.Name.Local {
			case "key":
				if err = decoder.DecodeElement(&key, &start); err != nil {
					t.Fatal(err)
				}
			case "string":
				var value string
				if err = decoder.DecodeElement(&value, &start); err != nil {
					t.Fatal(err)
				}
				values[key] = value
			case "true":
				booleans[key] = true
			}
		}
	}
	for key, expected := range map[string]string{"CFBundleVersion": info.Version, "CFBundleShortVersionString": info.Version, "CFBundleIdentifier": "com.kitonyterms.app", "CFBundleExecutable": "kitonyterms", "CFBundleIconFile": "candidate.icns", "LSMinimumSystemVersion": "15.0.0", "KitonyTermsBuildInfo": string(encoded)} {
		if values[key] != expected {
			t.Fatal("plist字段与身份不符", key)
		}
	}
	if !booleans["NSHighResolutionCapable"] || bytes.Contains(plist, []byte("My Company")) || bytes.Contains(plist, []byte("{{")) {
		t.Fatal("plist布尔或占位内容不符")
	}
	for _, invalid := range []string{string(encoded) + "{}", strings.Replace(string(encoded), info.Target, "x86_64-pc-windows-msvc", 1), strings.Replace(string(encoded), "0.1.18", "01.1.18", 1), strings.Replace(string(encoded), "stable", "development", 1)} {
		if _, err = parseIdentity([]byte(invalid)); err == nil {
			t.Fatal("无效或错误目标身份应拒绝")
		}
	}
}

func TestNewOutputAndArgumentRefusal(t *testing.T) {
	project, _ := filepath.EvalSymlinks(t.TempDir())
	for _, output := range []string{"relative", project, filepath.Join(project, "overlap")} {
		if _, err := newOutput(project, output); err == nil {
			t.Fatal("已有或重叠输出应拒绝")
		}
	}
	output := filepath.Join(t.TempDir(), "candidate 待验收")
	created, err := newOutput(project, output)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = newOutput(project, created); err == nil {
		t.Fatal("不能覆盖已有候选")
	}
	for _, args := range [][]string{nil, {"install"}, {"build"}, {"verify", "--candidate", "relative"}, {"build", "--unexpected"}} {
		if err = run(context.Background(), args, io.Discard); err == nil {
			t.Fatal("非法CLI应拒绝")
		}
	}
}

func TestBoundedInputsAndBundleShape(t *testing.T) {
	file := filepath.Join(t.TempDir(), "input")
	if err := os.WriteFile(file, []byte("12345"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := readBounded(file, 4); err == nil {
		t.Fatal("超限输入应拒绝")
	}
	if _, err := checkBinary(file); err == nil {
		t.Fatal("非Mach-O应拒绝")
	}
	if err := writeFile(file, []byte("overwrite"), 0600); err == nil {
		t.Fatal("写入不能覆盖")
	}
	bundle := t.TempDir()
	for _, name := range []string{plistName, iconName, executableName, "Contents/_CodeSignature/CodeResources"} {
		path := filepath.Join(bundle, name)
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte("public fixture"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := bundleHashes(bundle); err != nil {
		t.Fatal(err)
	}
	extra := filepath.Join(bundle, "Contents/Resources/unexpected")
	if err := os.WriteFile(extra, []byte("extra"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, err := bundleHashes(bundle); err == nil {
		t.Fatal("包内额外文件应拒绝")
	}
	if err := os.Remove(extra); err != nil {
		t.Fatal(err)
	}
	icon := filepath.Join(bundle, iconName)
	if err := os.Remove(icon); err != nil {
		t.Fatal(err)
	}
	if _, err := bundleHashes(bundle); err == nil {
		t.Fatal("缺少必要文件应拒绝")
	}
	if runtime.GOOS != "windows" {
		if err := os.Symlink(file, icon); err != nil {
			t.Fatal(err)
		}
		if _, err := bundleHashes(bundle); err == nil {
			t.Fatal("包内链接应拒绝")
		}
	}
}

func TestToolLimitsTimeoutAndCancellation(t *testing.T) {
	t.Setenv("KITONYTERMS_MACOS_PACKAGE_TEST_HELPER", "1")
	for _, mode := range []string{"stdout", "stderr", "sleep"} {
		t.Run(mode, func(t *testing.T) {
			t.Setenv("KITONYTERMS_MACOS_PACKAGE_TEST_MODE", mode)
			budget := 5 * time.Second
			if mode == "sleep" {
				budget = 200 * time.Millisecond
			}
			start := time.Now()
			stdout, stderr, err := runTool(context.Background(), budget, 4096, "", nil, os.Args[0])
			if err == nil || len(stdout) > 4096 || len(stderr) > 256<<10 || time.Since(start) > 6*time.Second {
				t.Fatal("输出或超时约束失效", err)
			}
			if mode == "sleep" && !errors.Is(err, context.DeadlineExceeded) {
				t.Fatal("超时根因丢失", err)
			}
		})
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, _, err := runTool(ctx, time.Second, 4096, "", nil, os.Args[0]); !errors.Is(err, context.Canceled) {
		t.Fatal("预取消应拒绝", err)
	}
}

func TestRuntimeIdentityAndZeroData(t *testing.T) {
	t.Setenv("KITONYTERMS_MACOS_PACKAGE_TEST_HELPER", "1")
	t.Setenv("KITONYTERMS_MACOS_PACKAGE_TEST_MODE", "identity")
	parent := t.TempDir()
	if err := verifyRuntime(context.Background(), parent, os.Args[0], testIdentity()); err != nil {
		t.Fatal(err)
	}
	entries, err := os.ReadDir(parent)
	if err != nil || len(entries) != 0 {
		t.Fatal("只读身份应回收空探测目录", err)
	}
	wrong := testIdentity()
	wrong.Attempt++
	if err := verifyRuntime(context.Background(), parent, os.Args[0], wrong); err == nil {
		t.Fatal("完整身份不符应拒绝")
	}
	t.Setenv("KITONYTERMS_MACOS_PACKAGE_TEST_MODE", "create")
	if err := verifyRuntime(context.Background(), parent, os.Args[0], testIdentity()); err == nil {
		t.Fatal("身份查询产生业务数据应拒绝")
	}
	entries, err = os.ReadDir(parent)
	if err != nil || len(entries) != 1 {
		t.Fatal("未知失败材料必须保留", err)
	}
}

func TestBuildCancellationAndFailureMaterials(t *testing.T) {
	if runtime.GOOS != "darwin" || runtime.GOARCH != "arm64" && runtime.GOARCH != "amd64" {
		t.Skip("实际候选失败构建需要macOS")
	}
	project := t.TempDir()
	icon, err := os.ReadFile("../../build/darwin/candidate.icns")
	if err != nil {
		t.Fatal(err)
	}
	for name, value := range map[string][]byte{"go.mod": []byte("module kitonyterms\n\ngo 1.26.0\n"), "go.sum": {}, "main.go": []byte("invalid Go source"), "internal/fixture/fixture.go": []byte("package fixture\n"), "frontend/dist/index.html": []byte("public"), resources[0]: icon} {
		path := filepath.Join(project, name)
		if err = os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(path, value, 0600); err != nil {
			t.Fatal(err)
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	output := filepath.Join(t.TempDir(), "pre-canceled")
	if _, err = buildCandidate(ctx, project, output, testIdentity()); !errors.Is(err, context.Canceled) {
		t.Fatal("预取消根因不符", err)
	}
	if _, err = os.Lstat(output); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("预取消不能创建输出")
	}
	output = filepath.Join(t.TempDir(), "failed")
	if _, err = buildCandidate(context.Background(), project, output, testIdentity()); err == nil {
		t.Fatal("错误源码应失败")
	}
	if _, err = os.Stat(filepath.Join(output, "build.log")); err != nil {
		t.Fatal("失败构建日志应保留", err)
	}
	if _, err = os.Lstat(filepath.Join(output, recordName)); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("失败不能发布成功记录")
	}
}
