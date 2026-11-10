package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"debug/buildinfo"
	"debug/pe"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"time"

	"github.com/tc-hib/winres"
	"github.com/tc-hib/winres/version"
	"kitonyterms/internal/update"
)

const maxResourceBytes = 8 << 20
const candidateObject = "kitonyterms_candidate_windows_amd64.syso"

// ResourceReport证明PE静态资源一致，不认证程序来源或证明Windows实际运行身份。
type ResourceReport struct {
	Identity                update.BuildInfo  `json:"identity"`
	Resources               map[string]string `json:"resources"`
	NumericVersion          string            `json:"numericVersion"`
	StaticResourcesVerified bool              `json:"staticResourcesVerified"`
	RuntimeIdentityVerified bool              `json:"runtimeIdentityVerified"`
}

type CandidateRecord struct {
	Schema    int               `json:"schema"`
	Status    string            `json:"status"`
	Resources ResourceReport    `json:"resourceCheck"`
	Files     map[string]string `json:"files"`
	Inputs    map[string]string `json:"inputs"`
}

func numericVersion(info update.BuildInfo) ([4]uint16, string, error) {
	var value [4]uint16
	if err := info.Validate(); err != nil {
		return value, "", err
	}
	for i, part := range strings.Split(info.Version, ".") {
		n, err := strconv.ParseUint(part, 10, 16)
		if err != nil {
			return value, "", errors.New("Windows数字版本每段必须在0–65535之间")
		}
		value[i] = uint16(n)
	}
	return value, info.Version + ".0", nil
}

func candidateResources(project string, info update.BuildInfo) (*winres.ResourceSet, []byte, []byte, error) {
	numbers, textVersion, err := numericVersion(info)
	if err != nil {
		return nil, nil, nil, err
	}
	icon, err := readBounded(filepath.Join(project, "build/windows/candidate.ico"), maxResourceBytes)
	if err != nil {
		return nil, nil, nil, err
	}
	ico, err := winres.LoadICO(bytes.NewReader(icon))
	if err != nil {
		return nil, nil, nil, err
	}
	resources := &winres.ResourceSet{}
	if err = resources.SetIcon(winres.RT_ICON, ico); err != nil {
		return nil, nil, nil, err
	}
	manifest, err := readBounded(filepath.Join(project, "build/windows/wails.exe.manifest"), 64<<10)
	if err != nil {
		return nil, nil, nil, err
	}
	parsed, err := winres.AppManifestFromXML(manifest)
	if err != nil {
		return nil, nil, nil, err
	}
	if parsed.Identity.Name != "com.kitonyterms.app" || parsed.ExecutionLevel != winres.AsInvoker || parsed.UIAccess || parsed.AutoElevate || parsed.DPIAwareness != winres.DPIPerMonitorV2 || !parsed.UseCommonControlsV6 {
		return nil, nil, nil, errors.New("候选manifest的应用身份/权限/DPI契约不匹配")
	}
	old := fmt.Sprintf(`name="com.kitonyterms.app" version="%d.%d.%d.%d"`, parsed.Identity.Version[0], parsed.Identity.Version[1], parsed.Identity.Version[2], parsed.Identity.Version[3])
	if bytes.Count(manifest, []byte(old)) != 1 {
		return nil, nil, nil, errors.New("manifest版本必须有唯一明确锚点")
	}
	manifest = bytes.Replace(manifest, []byte(old), []byte(`name="com.kitonyterms.app" version="`+textVersion+`"`), 1)
	// 保留原XML中的per-monitor回退，不让资源库重新拼装manifest。
	if err = resources.Set(winres.RT_MANIFEST, winres.ID(1), winres.LCIDDefault, manifest); err != nil {
		return nil, nil, nil, err
	}
	vi := version.Info{FileVersion: numbers, ProductVersion: numbers, Type: version.App}
	vi.Flags.Prerelease = info.Channel == "alpha"
	encodedIdentity, err := json.Marshal(info)
	if err != nil {
		return nil, nil, nil, err
	}
	for locale, description := range map[uint16]string{0x0409: "SSH and server management workspace", 0x0804: "跨平台 SSH 与服务器管理工作台"} {
		for key, value := range map[string]string{
			version.ProductName: "KitonyTerms", version.CompanyName: "Kitony", version.FileDescription: description,
			version.FileVersion: textVersion, version.ProductVersion: textVersion, version.InternalName: "kitonyterms",
			version.OriginalFilename: "kitonyterms.exe", version.LegalCopyright: "© 2026 KitonyTerms",
			version.Comments: string(encodedIdentity),
		} {
			if err = vi.Set(locale, key, value); err != nil {
				return nil, nil, nil, err
			}
		}
	}
	resources.SetVersionInfo(vi)
	versionJSON, err := json.MarshalIndent(vi, "", "  ")
	return resources, manifest, versionJSON, err
}

func resourceHashes(resources *winres.ResourceSet) map[string]string {
	result := map[string]string{}
	resources.Walk(func(kind, id winres.Identifier, locale uint16, data []byte) bool {
		hash := sha256.Sum256(data)
		result[fmt.Sprintf("%v/%v/%04x", kind, id, locale)] = hex.EncodeToString(hash[:])
		return true
	})
	return result
}

func verifyResources(project, binary, identityPath string) (report ResourceReport, err error) {
	project, err = absolute(project)
	if err != nil {
		return report, err
	}
	binary, err = absolute(binary)
	if err != nil {
		return report, err
	}
	identityPath, err = absolute(identityPath)
	if err != nil {
		return report, err
	}
	data, err := readBounded(identityPath, 4096)
	if err != nil {
		return report, err
	}
	info, err := identity(data)
	if err != nil {
		return report, err
	}
	if err = checkBinary(binary); err != nil {
		return report, err
	}
	build, err := buildinfo.ReadFile(binary)
	if err != nil {
		return report, err
	}
	pinned := false
	for _, dependency := range build.Deps {
		if dependency.Path == "github.com/wailsapp/wails/v3" {
			pinned = dependency.Version == "v3.0.0" && dependency.Replace == nil
		}
	}
	if !pinned {
		return report, errors.New("候选程序的Wails模块版本必须为v3.0.0且无替换")
	}
	expected, _, _, err := candidateResources(project, info)
	if err != nil {
		return report, err
	}
	image, err := pe.Open(binary)
	if err != nil {
		return report, err
	}
	defer func() { err = errors.Join(err, image.Close()) }()
	section := image.Section(".rsrc")
	if section == nil || section.Size == 0 || section.Size > maxResourceBytes {
		return report, errors.New("候选PE缺少有界资源段")
	}
	header, ok := image.OptionalHeader.(*pe.OptionalHeader64)
	if !ok || header.NumberOfRvaAndSizes <= pe.IMAGE_DIRECTORY_ENTRY_RESOURCE {
		return report, errors.New("候选PE缺少资源目录声明")
	}
	directory := header.DataDirectory[pe.IMAGE_DIRECTORY_ENTRY_RESOURCE]
	if directory.VirtualAddress != section.VirtualAddress || directory.Size == 0 || directory.Size > section.Size {
		return report, errors.New("PE资源目录必须绑定已校验的有界资源段")
	}
	for _, other := range image.Sections {
		if other != section && other.VirtualAddress == directory.VirtualAddress {
			return report, errors.New("PE资源段地址重复，拒绝不明确的读取")
		}
	}
	data, err = section.Data()
	if err != nil {
		return report, err
	}
	if err = validateResourceDirectory(data, section.VirtualAddress); err != nil {
		return report, err
	}
	file, err := os.Open(binary)
	if err != nil {
		return report, err
	}
	actual, loadErr := winres.LoadFromEXE(file)
	err = errors.Join(loadErr, file.Close())
	if err != nil {
		return report, err
	}
	if !reflect.DeepEqual(resourceHashes(expected), resourceHashes(actual)) {
		return report, errors.New("成品图标/双语版本/manifest资源与声明身份不一致")
	}
	_, numeric, err := numericVersion(info)
	return ResourceReport{Identity: info, Resources: resourceHashes(actual), NumericVersion: numeric, StaticResourcesVerified: true}, err
}

func buildCandidate(ctx context.Context, project, identityPath, output string) (record CandidateRecord, err error) {
	project, err = absolute(project)
	if err != nil {
		return record, err
	}
	identityPath, err = absolute(identityPath)
	if err != nil {
		return record, err
	}
	data, err := readBounded(identityPath, 4096)
	if err != nil {
		return record, err
	}
	info, err := identity(data)
	if err != nil {
		return record, err
	}
	mod, err := readBounded(filepath.Join(project, "go.mod"), 64<<10)
	if err != nil || strings.TrimSpace(strings.SplitN(string(mod), "\n", 2)[0]) != "module kitonyterms" {
		return record, errors.Join(errors.New("必须显式指定KitonyTerms app工程目录"), err)
	}
	if _, err = readBounded(filepath.Join(project, "frontend/dist/index.html"), 1<<20); err != nil {
		return record, errors.Join(errors.New("先完成前端生产构建"), err)
	}
	existing, err := filepath.Glob(filepath.Join(project, "*.syso"))
	if err != nil || len(existing) != 0 {
		return record, errors.Join(errors.New("工程根已有资源对象，拒绝不明确的合并"), err)
	}
	resources, manifest, versionJSON, err := candidateResources(project, info)
	if err != nil {
		return record, err
	}
	if err = ctx.Err(); err != nil {
		return record, err
	}
	if !filepath.IsAbs(output) {
		return record, errors.New("候选目录必须使用新绝对路径")
	}
	parent, err := absolute(filepath.Dir(filepath.Clean(output)))
	if err != nil {
		return record, err
	}
	output = filepath.Join(parent, filepath.Base(filepath.Clean(output)))
	if pathsOverlap(project, output) {
		return record, errors.New("候选目录不能与原工程重叠")
	}
	if err = os.Mkdir(output, 0700); err != nil {
		return record, err
	}
	inputProject := filepath.Join(output, "build-inputs")
	inputs, err := copyBuildInputs(ctx, project, inputProject)
	if err != nil {
		return record, err
	}
	resources, manifest, versionJSON, err = candidateResources(inputProject, info)
	if err != nil {
		return record, err
	}
	objectName := candidateObject
	arch := winres.ArchAMD64
	if info.Target == "aarch64-pc-windows-msvc" {
		objectName = "kitonyterms_candidate_windows_arm64.syso"
		arch = winres.ArchARM64
	}
	object, err := os.OpenFile(filepath.Join(inputProject, objectName), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return record, err
	}
	writeErr := resources.WriteObject(object, arch)
	if writeErr == nil {
		writeErr = object.Sync()
	}
	if err = errors.Join(writeErr, object.Close()); err != nil {
		return record, err
	}
	encoded, err := json.Marshal(info)
	if err != nil {
		return record, err
	}
	inputJSON, err := json.MarshalIndent(inputs, "", "  ")
	if err != nil {
		return record, err
	}
	for name, content := range map[string][]byte{"build-inputs.json": inputJSON, "manifest.xml": manifest, "version.json": versionJSON, "identity.json": encoded} {
		if err = writeFile(filepath.Join(output, name), bytes.NewReader(content), 0600); err != nil {
			return record, err
		}
	}
	goTool, err := exec.LookPath("go")
	if err != nil {
		return record, err
	}
	buildContext, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()
	binary := filepath.Join(output, "kitonyterms.exe")
	flags := "-w -s -H windowsgui -X kitonyterms/internal/update.embeddedIdentity=" + base64.RawURLEncoding.EncodeToString(encoded)
	command := exec.CommandContext(buildContext, goTool, "build", "-mod=readonly", "-tags=production", "-trimpath", "-buildvcs=false", "-ldflags="+flags, "-o", binary, ".")
	command.Env, command.Dir, command.WaitDelay = candidateEnvironmentFor(string(arch)), inputProject, time.Second
	var diagnostics boundedOutput
	command.Stdout, command.Stderr = &diagnostics, &diagnostics
	runErr := command.Run()
	logErr := writeFile(filepath.Join(output, "build.log"), bytes.NewReader(diagnostics.buffer.Bytes()), 0600)
	if err = errors.Join(runErr, buildContext.Err(), logErr); err != nil {
		return record, fmt.Errorf("候选构建失败，隔离材料与有界build.log保留：%w", err)
	}
	report, err := verifyResources(inputProject, binary, identityPath)
	if err != nil || report.Identity != info {
		return record, errors.Join(errors.New("候选构建后的身份/资源回读失败"), err)
	}
	for _, directory := range []string{project, inputProject} {
		if err = verifyBuildInputs(ctx, directory, inputs); err != nil {
			return record, err
		}
	}
	record = CandidateRecord{Schema: 1, Status: "built_static_verified", Resources: report, Inputs: inputs, Files: map[string]string{}}
	for _, name := range []string{"build-inputs/" + objectName, "build-inputs.json", "manifest.xml", "version.json", "identity.json", "build.log", "kitonyterms.exe"} {
		record.Files[name], err = digestFile(filepath.Join(output, name))
		if err != nil {
			return record, err
		}
	}
	if err = ctx.Err(); err != nil {
		return record, err
	}
	data, err = json.MarshalIndent(record, "", "  ")
	if err != nil {
		return record, err
	}
	if err = writeFile(filepath.Join(output, "candidate-build.json"), bytes.NewReader(append(data, '\n')), 0600); err != nil {
		return record, err
	}
	return record, nil
}

func candidateEnvironment() []string {
	return candidateEnvironmentFor("amd64")
}

func candidateEnvironmentFor(arch string) []string {
	fixed := map[string]string{"GOOS": "windows", "GOARCH": arch, "CGO_ENABLED": "0", "GOWORK": "off", "GOFLAGS": ""}
	var result []string
	for _, item := range os.Environ() {
		key, _, _ := strings.Cut(item, "=")
		if _, replace := fixed[strings.ToUpper(key)]; !replace {
			result = append(result, item)
		}
	}
	for key, value := range fixed {
		result = append(result, key+"="+value)
	}
	return result
}
