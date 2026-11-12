package main

import (
	"bytes"
	"context"
	"debug/pe"
	"encoding/binary"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"strings"
	"testing"

	"github.com/tc-hib/winres"
	"github.com/tc-hib/winres/version"
)

func resourceProject(t *testing.T) string {
	t.Helper()
	project := t.TempDir()
	for _, name := range []string{"candidate.ico", "wails.exe.manifest"} {
		value, err := os.ReadFile(filepath.Join("../../build/windows", name))
		if err != nil {
			t.Fatal(err)
		}
		path := filepath.Join(project, "build/windows", name)
		if err = os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(path, value, 0600); err != nil {
			t.Fatal(err)
		}
	}
	return project
}

func TestCandidateVersionAndBilingualResources(t *testing.T) {
	info := testIdentity()
	info.Build, info.Attempt = 1<<63, 1<<62
	for _, channel := range []string{"stable", "alpha"} {
		info.Channel = channel
		resources, manifest, _, err := candidateResources(resourceProject(t), info)
		if err != nil {
			t.Fatal(err)
		}
		if !bytes.Contains(manifest, []byte(`version="0.1.18.0"`)) || !bytes.Contains(manifest, []byte("permonitorv2,permonitor")) || !bytes.Contains(manifest, []byte(`level="asInvoker"`)) {
			t.Fatal("manifest版本/权限/原DPI回退不符")
		}
		for _, locale := range []uint16{0x0409, 0x0804} {
			vi, err := version.FromBytes(resources.Get(winres.RT_VERSION, winres.ID(1), locale))
			if err != nil {
				t.Fatal(err)
			}
			if vi.FileVersion != [4]uint16{0, 1, 18, 0} || vi.ProductVersion != vi.FileVersion || vi.Flags.Prerelease != (channel == "alpha") {
				t.Fatal("数字版本/Alpha标记不符")
			}
			table := vi.Table()[locale]
			if table == nil || (*table)[version.ProductName] != productDisplayName || (*table)[version.FileDescription] != productDisplayName || (*table)[version.CompanyName] != "Kitony" || (*table)[version.OriginalFilename] != "kitonyterms.exe" {
				t.Fatal("双语产品字段缺失或显示名不一致")
			}
			_, textVersion, err := numericVersion(info)
			if err != nil {
				t.Fatal(err)
			}
			if (*table)[version.FileVersion] != textVersion || (*table)[version.ProductVersion] != textVersion {
				t.Fatal("字符串版本字段未跟随构建身份")
			}
			data, _ := json.Marshal(info)
			if (*table)[version.Comments] != string(data) {
				t.Fatal("完整大构建号/commit身份不能截断")
			}
		}
	}
	for _, value := range []string{"65536.1.2", "1.65536.2", "1.2.65536", "18446744073709551615.0.0"} {
		info.Version = value
		if _, _, err := numericVersion(info); err == nil {
			t.Fatal("Windows16位数字版本应拒绝超限")
		}
	}
}

func releaseVersion(t *testing.T) string {
	t.Helper()
	manifest, err := os.ReadFile("../../frontend/package.json")
	if err != nil {
		t.Fatal(err)
	}
	var project struct {
		Version string `json:"version"`
	}
	if err := json.Unmarshal(manifest, &project); err != nil {
		t.Fatal(err)
	}
	if !regexp.MustCompile(`^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$`).MatchString(project.Version) {
		t.Fatalf("发布版本必须是三段数字：%q", project.Version)
	}
	return project.Version
}

func TestDisplayNameContract(t *testing.T) {
	files := []string{
		"../../build/config.yml",
		"../../build/windows/info.json",
		"../../build/ios/Info.plist",
		"../../main.go",
	}
	forbidden := []string{"跨平台 SSH 与服务器管理工作台", "跨平台 SSH 工作台", "SSH and server management workspace", "Wails 迁移版"}
	for _, name := range files {
		data, err := os.ReadFile(name)
		if err != nil {
			t.Fatal(err)
		}
		for _, value := range forbidden {
			if bytes.Contains(data, []byte(value)) {
				t.Fatalf("%s仍包含旧显示名%q", name, value)
			}
		}
	}
	main, err := os.ReadFile("../../main.go")
	if err != nil {
		t.Fatal(err)
	}
	for _, fragment := range []string{`const appDisplayName = "KitonyTerms"`, "Name: appDisplayName", "Description: appDisplayName", "Title: appDisplayName"} {
		if !bytes.Contains(main, []byte(fragment)) {
			t.Fatalf("main.go缺少统一显示名引用：%s", fragment)
		}
	}
	info, err := os.ReadFile("../../build/windows/info.json")
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Count(info, []byte(`"FileDescription": "`+productDisplayName+`"`)) != 2 {
		t.Fatal("两种语言的Windows资源描述必须统一为KitonyTerms")
	}
	plist, err := os.ReadFile("../../build/ios/Info.plist")
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(plist, []byte("<string>"+productDisplayName+" "+releaseVersion(t)+"</string>")) {
		t.Fatal("iOS信息串必须使用KitonyTerms加版本号")
	}
}

func TestCandidateRejectsUnsafeManifest(t *testing.T) {
	for _, change := range []struct{ before, after string }{{`level="asInvoker"`, `level="requireAdministrator"`}, {`uiAccess="false"`, `uiAccess="true"`}, {"permonitorv2,permonitor", "system"}, {"com.kitonyterms.app", "foreign.application"}} {
		project := resourceProject(t)
		path := filepath.Join(project, "build/windows/wails.exe.manifest")
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(path, []byte(strings.ReplaceAll(string(data), change.before, change.after)), 0600); err != nil {
			t.Fatal(err)
		}
		if _, _, _, err = candidateResources(project, testIdentity()); err == nil {
			t.Fatal("身份/权限/DPI变更应拒绝")
		}
	}
}

func minimalResourceDirectory() []byte {
	data := make([]byte, 96)
	for _, offset := range []int{0, 24, 48} {
		binary.LittleEndian.PutUint16(data[offset+14:], 1)
	}
	binary.LittleEndian.PutUint32(data[16:], 24)
	binary.LittleEndian.PutUint32(data[20:], 0x80000018)
	binary.LittleEndian.PutUint32(data[40:], 1)
	binary.LittleEndian.PutUint32(data[44:], 0x80000030)
	binary.LittleEndian.PutUint32(data[64:], 0) // 合法的中性语言
	binary.LittleEndian.PutUint32(data[68:], 72)
	binary.LittleEndian.PutUint32(data[72:], 0x1000+88)
	binary.LittleEndian.PutUint32(data[76:], 8)
	return data
}

func TestResourceDirectoryBounds(t *testing.T) {
	if err := validateResourceDirectory(minimalResourceDirectory(), 0x1000); err != nil {
		t.Fatal("合法中性语言目录未通过", err)
	}
	for _, mutate := range []func([]byte){func(d []byte) { binary.LittleEndian.PutUint16(d[12:], 1) }, func(d []byte) { binary.LittleEndian.PutUint16(d[14:], 513) }, func(d []byte) { binary.LittleEndian.PutUint32(d[20:], 0x80000000) }, func(d []byte) { binary.LittleEndian.PutUint32(d[68:], 0x80000048) }, func(d []byte) { binary.LittleEndian.PutUint32(d[72:], 0xfff) }, func(d []byte) { binary.LittleEndian.PutUint32(d[76:], 0xffffffff) }} {
		data := minimalResourceDirectory()
		mutate(data)
		if err := validateResourceDirectory(data, 0x1000); err == nil {
			t.Fatal("损坏或超限资源目录应拒绝")
		}
	}
	for size := 0; size < 96; size++ {
		if err := validateResourceDirectory(minimalResourceDirectory()[:size], 0x1000); err == nil {
			t.Fatal("截断目录应拒绝")
		}
	}
}

func TestCandidatePreflightAndFailedBuildKeepMaterials(t *testing.T) {
	project := resourceProject(t)
	for name, value := range map[string]string{"go.mod": "module kitonyterms\r\n\r\ngo 1.26.0\r\n", "go.sum": "", "main.go": "package main\nfunc main(){ unresolved }\n", "internal/fixture.go": "package fixture\n", "frontend/dist/index.html": "public fixture"} {
		path := filepath.Join(project, name)
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte(value), 0600); err != nil {
			t.Fatal(err)
		}
	}
	identity := identityFile(t, testIdentity())
	output := filepath.Join(t.TempDir(), "failed")
	if _, err := buildCandidate(context.Background(), project, identity, output); err == nil {
		t.Fatal("无效源应使构建失败")
	}
	if _, err := os.Stat(filepath.Join(output, "build-inputs", candidateObject)); err != nil {
		t.Fatal("失败资源材料应保留")
	}
	if _, err := os.Stat(filepath.Join(output, "candidate-build.json")); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("失败不能记录成功")
	}
	if _, err := os.Stat(filepath.Join(project, candidateObject)); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("隔离构建不能写入原源码资源对象")
	}
	before, err := os.ReadFile(filepath.Join(output, "build-inputs.json"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err = buildCandidate(context.Background(), project, identity, output); err == nil {
		t.Fatal("不能复用或覆盖已有目录")
	}
	after, _ := os.ReadFile(filepath.Join(output, "build-inputs.json"))
	if !bytes.Equal(before, after) {
		t.Fatal("已有材料改变")
	}
	if err = os.WriteFile(filepath.Join(project, "foreign.syso"), []byte("keep"), 0600); err != nil {
		t.Fatal(err)
	}
	refused := filepath.Join(t.TempDir(), "refused")
	if _, err = buildCandidate(context.Background(), project, identity, refused); err == nil {
		t.Fatal("已有syso应拒绝不明确合并")
	}
	if _, err = os.Stat(refused); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("预检失败不创建目录")
	}
}

func TestActualCandidateResourceReadbackAndTamper(t *testing.T) {
	binaryPath := os.Getenv("KITONYTERMS_WINDOWS_RESOURCE_BINARY")
	identityPath := os.Getenv("KITONYTERMS_WINDOWS_RESOURCE_IDENTITY")
	if binaryPath == "" || identityPath == "" {
		t.Skip("需要本批实际Windows候选PE，不计作Windows执行")
	}
	project, err := filepath.Abs("../..")
	if err != nil {
		t.Fatal(err)
	}
	report, err := verifyResources(project, binaryPath, identityPath)
	if err != nil || !report.StaticResourcesVerified || report.RuntimeIdentityVerified {
		t.Fatal("实际成品资源未通过或被误记为原生执行", err)
	}
	identityData, err := os.ReadFile(identityPath)
	if err != nil {
		t.Fatal(err)
	}
	mismatch, err := identity(identityData)
	if err != nil {
		t.Fatal(err)
	}
	mismatch.Build++
	if _, err = verifyResources(project, binaryPath, identityFile(t, mismatch)); err == nil {
		t.Fatal("实际PE的完整声明身份变化应拒绝")
	}
	copied := filepath.Join(t.TempDir(), "changed.exe")
	data, err := os.ReadFile(binaryPath)
	if err != nil {
		t.Fatal(err)
	}
	image, err := pe.Open(binaryPath)
	if err != nil {
		t.Fatal(err)
	}
	section := image.Section(".rsrc")
	if section == nil {
		t.Fatal("实际PE缺少资源")
	}
	offset := int(section.Offset)
	peOffset := int(binary.LittleEndian.Uint32(data[0x3c:]))
	directoryOffset := peOffset + 24 + 112 + pe.IMAGE_DIRECTORY_ENTRY_RESOURCE*8
	textAddress, textHeaderOffset := uint32(0), 0
	for i, other := range image.Sections {
		if other.Name == ".text" {
			textAddress = other.VirtualAddress
			textHeaderOffset = peOffset + 24 + int(image.SizeOfOptionalHeader) + i*40 + 12
		}
	}
	if textHeaderOffset == 0 {
		t.Fatal("实际PE缺少代码段夹具")
	}
	if err = image.Close(); err != nil {
		t.Fatal(err)
	}
	for _, change := range []struct {
		offset int
		value  uint32
		reason string
	}{{directoryOffset, textAddress, "绑定已校验"}, {directoryOffset + 4, maxResourceBytes + 1, "绑定已校验"}, {textHeaderOffset, section.VirtualAddress, "地址重复"}} {
		changed := append([]byte(nil), data...)
		binary.LittleEndian.PutUint32(changed[change.offset:], change.value)
		if err = os.WriteFile(copied, changed, 0600); err != nil {
			t.Fatal(err)
		}
		if _, err = verifyResources(project, copied, identityPath); err == nil || !strings.Contains(err.Error(), change.reason) {
			t.Fatal("资源目录/段绑定篡改须在调用资源库前拒绝", err)
		}
	}
	data[offset+14], data[offset+15] = 0xff, 0xff
	if err = os.WriteFile(copied, data, 0600); err != nil {
		t.Fatal(err)
	}
	if _, err = verifyResources(project, copied, identityPath); err == nil {
		t.Fatal("实际PE资源目录损坏应拒绝")
	}
	if reflect.DeepEqual(report.Resources, map[string]string{}) {
		t.Fatal("缺少实际资源摘要")
	}
	t.Log("实际PE图标、双语版本、完整身份及manifest回读通过，Windows程序未执行")
}
