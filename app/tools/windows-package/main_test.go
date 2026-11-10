package main

import (
	"archive/zip"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
	"time"

	"kitonyterms/internal/update"
)

func testIdentity() update.BuildInfo {
	return update.BuildInfo{Channel: "stable", Version: "0.1.18", Commit: strings.Repeat("c", 40), Build: 602, Attempt: 1, Target: "x86_64-pc-windows-msvc"}
}
func identityFile(t *testing.T, info update.BuildInfo) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "identity.json")
	data, err := json.Marshal(info)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(path, data, 0600); err != nil {
		t.Fatal(err)
	}
	return path
}
func binaryFixture(t *testing.T) string {
	t.Helper()
	path := os.Getenv("KITONYTERMS_WINDOWS_PACKAGE_BINARY")
	if path == "" {
		t.Skip("实际Windows候选PE需由独立交叉构建提供，非目标执行")
	}
	return path
}
func TestIdentityAndInnoContract(t *testing.T) {
	info := testIdentity()
	data, _ := json.Marshal(info)
	actual, err := identity(data)
	if err != nil || actual != info {
		t.Fatal("有效身份未通过", err)
	}
	for _, value := range [][]byte{append(data, []byte("{}")...), []byte(`{"unknown":true}`), bytes.Repeat([]byte("x"), 4097)} {
		if _, err := identity(value); err == nil {
			t.Fatal("无效身份应拒绝")
		}
	}
	for _, mutate := range []func(*update.BuildInfo){func(i *update.BuildInfo) { i.Channel = "development" }, func(i *update.BuildInfo) { i.Target = "aarch64-apple-darwin" }, func(i *update.BuildInfo) { i.Version = "0.1.18\n[Run]" }, func(i *update.BuildInfo) { i.Commit = "" }} {
		changed := info
		mutate(&changed)
		data, _ := json.Marshal(changed)
		if _, err := identity(data); err == nil {
			t.Fatal("身份边界未拒绝")
		}
	}
	text := script(info)
	for _, value := range []string{"AppId=KitonyTerms\n", "PrivilegesRequired=admin\n", "ArchitecturesInstallIn64BitMode=\n", "CloseApplications=no\n", "RestartApplications=no\n", "AppVersion=0.1.18\n", "OutputBaseFilename=kitonyterms-v0.1.18-windows-x64-setup\n", "kitonyterms-installation.txt", "skipifsilent"} {
		if !strings.Contains(text, value) {
			t.Fatal("Inno契约缺失")
		}
	}
	info.Channel = "alpha"
	if baseName(info) != "kitonyterms-alpha-windows-x64" {
		t.Fatal("Alpha资产名称无效")
	}
}
func TestPreparedWindowsPackageRoundtrip(t *testing.T) {
	binary := binaryFixture(t)
	before, err := digestFile(binary)
	if err != nil {
		t.Fatal(err)
	}
	output := filepath.Join(t.TempDir(), "prepared")
	plan, err := prepare(binary, identityFile(t, testIdentity()), output)
	if err != nil {
		t.Fatal(err)
	}
	verified, err := verifyPrepared(output)
	if err != nil || !reflect.DeepEqual(verified, plan) {
		t.Fatal("暂存回读失败", err)
	}
	archive, err := zip.OpenReader(filepath.Join(output, baseName(plan.Identity)+".zip"))
	if err != nil {
		t.Fatal(err)
	}
	if len(archive.File) != 1 || archive.File[0].Name != "kitonyterms.exe" {
		t.Fatal("便携包混入安装标记或其它文件")
	}
	if err = archive.Close(); err != nil {
		t.Fatal(err)
	}
	if after, err := digestFile(binary); err != nil || after != before {
		t.Fatal("源候选程序被修改")
	}
	if _, err = prepare(binary, identityFile(t, testIdentity()), output); err == nil {
		t.Fatal("已有暂存目录应拒绝")
	}
	if _, err = verifyPrepared(output); err != nil {
		t.Fatal("已有材料不应受影响")
	}
	t.Log("实际Windows Wails PE已暂存及ZIP回读，未执行Windows程序")
}
func TestPreparedPackageRejectsTamperAndInvalidDeclaredIdentity(t *testing.T) {
	binary := binaryFixture(t)
	changed := testIdentity()
	changed.Build = 0
	out := filepath.Join(t.TempDir(), "not-created")
	if _, err := prepare(binary, identityFile(t, changed), out); err == nil {
		t.Fatal("无效声明身份应拒绝")
	}
	if _, err := os.Stat(out); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("身份失败不应创建目标")
	}
	for _, name := range []string{binaryName, markerName, "project.iss", baseName(testIdentity()) + ".zip", planName} {
		t.Run(name, func(t *testing.T) {
			out := filepath.Join(t.TempDir(), "prepared")
			if _, err := prepare(binary, identityFile(t, testIdentity()), out); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(out, name), []byte("corrupted"), 0600); err != nil {
				t.Fatal(err)
			}
			if _, err := verifyPrepared(out); err == nil {
				t.Fatal("篡改暂存材料应拒绝")
			}
		})
	}
}
func TestPackageRejectsNonPEAndArguments(t *testing.T) {
	binary := filepath.Join(t.TempDir(), "foreign.exe")
	if err := os.WriteFile(binary, []byte("not-pe"), 0600); err != nil {
		t.Fatal(err)
	}
	out := filepath.Join(t.TempDir(), "not-created")
	if _, err := prepare(binary, identityFile(t, testIdentity()), out); err == nil {
		t.Fatal("非PE应拒绝")
	}
	if _, err := os.Stat(out); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("无效输入不应创建目标")
	}
	for _, args := range [][]string{nil, {"unknown"}, {"prepare", "--binary"}, {"verify", "--prepared", "private", "--prepared", "second"}, {"compile", "--prepared", "private", "--bad", "value"}} {
		var output bytes.Buffer
		if err := run(context.Background(), args, &output); err == nil || output.Len() != 0 || strings.Contains(err.Error(), "private") {
			t.Fatal("参数验证或输出边界失败")
		}
	}
	if runtime.GOOS != "windows" {
		if err := compile(context.Background(), out, "ISCC.exe"); err == nil || !strings.Contains(err.Error(), "Windows原生") {
			t.Fatal("非Windows不能执行编译器")
		}
	}
}

type onlyReader struct{ io.Reader }

func TestToolOutputBoundCannotBypassReaderFrom(t *testing.T) {
	var output boundedOutput
	if _, err := io.Copy(&output, onlyReader{strings.NewReader(strings.Repeat("x", (256<<10)+1))}); err == nil || output.buffer.Len() > 256<<10 {
		t.Fatal("io.Copy绕过了输出边界")
	}
}
func TestPackageToolChild(t *testing.T) {
	switch os.Getenv("KITONYTERMS_PACKAGE_CHILD") {
	case "output":
		os.Stdout.WriteString(strings.Repeat("x", (256<<10)+1))
		os.Exit(0)
	case "wait":
		os.Stdout.WriteString("ready\n")
		time.Sleep(time.Hour)
		os.Exit(0)
	}
}
func TestNativeToolOutputAndTimeoutWaitActualProcess(t *testing.T) {
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	for _, mode := range []string{"output", "wait"} {
		t.Run(mode, func(t *testing.T) {
			t.Setenv("KITONYTERMS_PACKAGE_CHILD", mode)
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			if mode == "wait" {
				cancel()
				ctx, cancel = context.WithTimeout(context.Background(), 150*time.Millisecond)
			}
			defer cancel()
			value, err := execute(ctx, t.TempDir(), executable, "-test.run=^TestPackageToolChild$")
			if err == nil || mode == "wait" && (!errors.Is(err, context.DeadlineExceeded) || !bytes.Contains(value, []byte("ready\n"))) {
				t.Fatal("输出超限/超时应失败并等待实际子进程退出")
			}
		})
	}
}
func TestPackageRejectsFIFOInputBeforeOpening(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("FIFO仅Unix")
	}
	path := filepath.Join(t.TempDir(), "identity")
	if err := exec.Command("mkfifo", path).Run(); err != nil {
		t.Fatal(err)
	}
	if _, err := readBounded(path, 4096); err == nil {
		t.Fatal("FIFO必须在打开前拒绝")
	}
}
