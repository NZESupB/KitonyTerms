// 独立候选包工具；不切换正式发布入口，不安装或签名应用。
package main

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"debug/buildinfo"
	"debug/pe"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"kitonyterms/internal/update"
)

//go:embed inno/project.iss
var innoTemplate string

const marker = "KitonyTerms Inno Setup installation v1\r\n"
const planName = "package-plan.json"
const binaryName = "payload/kitonyterms.exe"
const markerName = "payload/kitonyterms-installation.txt"

// Plan仅记录公开的声明身份和相对产物摘要；Windows编译前另查实际运行身份。
type Plan struct {
	Schema         int               `json:"schema"`
	Identity       update.BuildInfo  `json:"identity"`
	Files          map[string]string `json:"files"`
	IdentityStatus string            `json:"identityStatus"`
}

func readJSON(data []byte, target any) error {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(target); err != nil {
		return errors.New("构建身份或暂存清单格式无效")
	}
	var extra any
	if decoder.Decode(&extra) != io.EOF {
		return errors.New("构建身份或暂存清单含多余内容")
	}
	return nil
}
func identity(data []byte) (info update.BuildInfo, err error) {
	if len(data) > 4096 {
		return info, errors.New("构建身份超限")
	}
	if err = readJSON(data, &info); err != nil {
		return info, err
	}
	if err = info.Validate(); err != nil {
		return info, err
	}
	if info.Target != "x86_64-pc-windows-msvc" && info.Target != "aarch64-pc-windows-msvc" {
		return info, errors.New("此打包入口仅支持Windows amd64/ARM64")
	}
	return info, nil
}
func absolute(path string) (string, error) {
	if !filepath.IsAbs(path) {
		return "", errors.New("路径必须为显式绝对路径")
	}
	info, err := os.Lstat(path)
	if err != nil {
		return "", err
	}
	if info.Mode()&os.ModeSymlink != 0 {
		return "", errors.New("拒绝链接输入")
	}
	return filepath.EvalSymlinks(path)
}
func readBounded(path string, limit int64) ([]byte, error) {
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() || info.Size() > limit {
		return nil, errors.Join(errors.New("输入必须是有界普通文件"), err)
	}
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	value, readErr := io.ReadAll(io.LimitReader(file, limit+1))
	if len(value) > int(limit) {
		readErr = errors.New("文件超限")
	}
	return value, errors.Join(readErr, file.Close())
}
func checkBinary(path string) error {
	info, err := os.Lstat(path)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() || info.Size() <= 0 || uint64(info.Size()) > update.MaxPackage {
		return errors.New("候选程序类型或体积无效")
	}
	image, err := pe.Open(path)
	if err != nil {
		return errors.New("候选程序不是PE文件")
	}
	header, ok := image.OptionalHeader.(*pe.OptionalHeader64)
	machine := image.Machine
	valid := ok && (machine == pe.IMAGE_FILE_MACHINE_AMD64 || machine == pe.IMAGE_FILE_MACHINE_ARM64) && header.Subsystem == pe.IMAGE_SUBSYSTEM_WINDOWS_GUI && image.Characteristics&pe.IMAGE_FILE_DLL == 0
	if err = image.Close(); err != nil {
		return err
	}
	if !valid {
		return errors.New("候选程序必须是Windows amd64/ARM64 GUI可执行文件")
	}
	build, err := buildinfo.ReadFile(path)
	if err != nil || build.Path != "kitonyterms" || build.Main.Path != "kitonyterms" {
		return errors.New("候选程序缺少KitonyTerms Go构建信息")
	}
	settings := map[string]string{}
	for _, setting := range build.Settings {
		settings[setting.Key] = setting.Value
	}
	expectedArch := map[uint16]string{pe.IMAGE_FILE_MACHINE_AMD64: "amd64", pe.IMAGE_FILE_MACHINE_ARM64: "arm64"}[machine]
	if settings["GOOS"] != "windows" || settings["GOARCH"] != expectedArch {
		return errors.New("Go构建目标不匹配")
	}
	production := false
	for _, tag := range strings.Split(settings["-tags"], ",") {
		if tag == "server" {
			return errors.New("server构建不能作为桌面安装载荷")
		}
		production = production || tag == "production"
	}
	if !production {
		return errors.New("候选程序缺少production标签")
	}
	return nil
}
func baseName(info update.BuildInfo) string {
	version := "v" + info.Version
	if info.Channel == "alpha" {
		version = "alpha"
	}
	arch := "x64"
	if info.Target == "aarch64-pc-windows-msvc" {
		arch = "aarch64"
	}
	return "kitonyterms-" + version + "-windows-" + arch
}
func script(info update.BuildInfo) string {
	text := strings.NewReplacer("{{VERSION}}", info.Version, "{{BASENAME}}", baseName(info)).Replace(innoTemplate)
	if info.Target == "aarch64-pc-windows-msvc" {
		text = strings.Replace(text, "ArchitecturesAllowed=x64compatible", "ArchitecturesAllowed=arm64", 1)
	}
	return text
}
func digestFile(path string) (string, error) {
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() || uint64(info.Size()) > update.MaxPackage {
		return "", errors.Join(errors.New("产物必须是有界普通文件"), err)
	}
	file, err := os.Open(path)
	if err != nil {
		return "", err
	}
	hash := sha256.New()
	count, readErr := io.Copy(hash, io.LimitReader(file, int64(update.MaxPackage)+1))
	if uint64(count) > update.MaxPackage {
		readErr = errors.New("产物超限")
	}
	return hex.EncodeToString(hash.Sum(nil)), errors.Join(readErr, file.Close())
}
func writeFile(path string, reader io.Reader, mode os.FileMode) error {
	file, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, mode)
	if err != nil {
		return err
	}
	_, writeErr := io.Copy(file, reader)
	if writeErr == nil {
		writeErr = file.Sync()
	}
	return errors.Join(writeErr, file.Close())
}
func portableZip(output, binary string) (err error) {
	file, err := os.OpenFile(output, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	archive := zip.NewWriter(file)
	header := &zip.FileHeader{Name: "kitonyterms.exe", Method: zip.Deflate}
	header.SetMode(0700)
	writer, createErr := archive.CreateHeader(header)
	if createErr == nil {
		input, openErr := os.Open(binary)
		if openErr == nil {
			_, createErr = io.Copy(writer, input)
			createErr = errors.Join(createErr, input.Close())
		} else {
			createErr = openErr
		}
	}
	if err = errors.Join(createErr, archive.Close()); err != nil {
		return err
	}
	return file.Sync()
}
func prepare(binary, identityPath, output string) (result Plan, err error) {
	binary, err = absolute(binary)
	if err != nil {
		return result, err
	}
	identityPath, err = absolute(identityPath)
	if err != nil {
		return result, err
	}
	value, err := readBounded(identityPath, 4096)
	if err != nil {
		return result, err
	}
	info, err := identity(value)
	if err != nil {
		return result, err
	}
	if err = checkBinary(binary); err != nil {
		return result, err
	}
	before, err := digestFile(binary)
	if err != nil {
		return result, err
	}
	if !filepath.IsAbs(output) {
		return result, errors.New("新暂存目录必须使用绝对路径")
	}
	parent, err := absolute(filepath.Dir(filepath.Clean(output)))
	if err != nil {
		return result, err
	}
	output = filepath.Join(parent, filepath.Base(filepath.Clean(output)))
	if err = os.Mkdir(output, 0700); err != nil {
		return result, err
	}
	if err = os.Mkdir(filepath.Join(output, "payload"), 0700); err != nil {
		return result, err
	}
	input, err := os.Open(binary)
	if err != nil {
		return result, err
	}
	err = errors.Join(writeFile(filepath.Join(output, binaryName), io.LimitReader(input, int64(update.MaxPackage)+1), 0700), input.Close())
	if err != nil {
		return result, err
	}
	copied, err := digestFile(filepath.Join(output, binaryName))
	if err != nil || copied != before {
		return result, errors.Join(errors.New("复制期间候选程序变化"), err)
	}
	if err = checkBinary(filepath.Join(output, binaryName)); err != nil {
		return result, err
	}
	if err = portableZip(filepath.Join(output, baseName(info)+".zip"), filepath.Join(output, binaryName)); err != nil {
		return result, err
	}
	for name, text := range map[string]string{markerName: marker, "project.iss": script(info)} {
		if err = writeFile(filepath.Join(output, name), strings.NewReader(text), 0600); err != nil {
			return result, err
		}
	}
	result = Plan{Schema: 1, Identity: info, IdentityStatus: "declared", Files: map[string]string{}}
	for _, name := range []string{binaryName, markerName, "project.iss", baseName(info) + ".zip"} {
		result.Files[name], err = digestFile(filepath.Join(output, name))
		if err != nil {
			return result, err
		}
	}
	data, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		return result, err
	}
	if err = writeFile(filepath.Join(output, planName), bytes.NewReader(append(data, '\n')), 0600); err != nil {
		return result, err
	}
	return verifyPrepared(output)
}
func verifyPrepared(path string) (plan Plan, err error) {
	path, err = absolute(path)
	if err != nil {
		return plan, err
	}
	payload, err := os.Lstat(filepath.Join(path, "payload"))
	if err != nil || !payload.IsDir() || payload.Mode()&os.ModeSymlink != 0 {
		return plan, errors.Join(errors.New("载荷目录必须是实际目录"), err)
	}
	value, err := readBounded(filepath.Join(path, planName), 16384)
	if err != nil {
		return plan, err
	}
	if err = readJSON(value, &plan); err != nil {
		return plan, err
	}
	encoded, err := json.Marshal(plan.Identity)
	if err != nil {
		return plan, err
	}
	if _, err = identity(encoded); err != nil || plan.Schema != 1 || plan.IdentityStatus != "declared" || len(plan.Files) != 4 {
		return plan, errors.Join(errors.New("暂存清单无效"), err)
	}
	for _, name := range []string{binaryName, markerName, "project.iss", baseName(plan.Identity) + ".zip"} {
		actual, hashErr := digestFile(filepath.Join(path, name))
		if hashErr != nil || plan.Files[name] != actual {
			return plan, errors.Join(errors.New("暂存产物摘要不匹配"), hashErr)
		}
	}
	text, err := readBounded(filepath.Join(path, "project.iss"), 16384)
	if err != nil || string(text) != script(plan.Identity) {
		return plan, errors.Join(errors.New("Inno模板不匹配"), err)
	}
	text, err = readBounded(filepath.Join(path, markerName), 256)
	if err != nil || string(text) != marker {
		return plan, errors.Join(errors.New("安装标记不匹配"), err)
	}
	if err = checkBinary(filepath.Join(path, binaryName)); err != nil {
		return plan, err
	}
	return plan, verifyZip(filepath.Join(path, baseName(plan.Identity)+".zip"), plan.Files[binaryName])
}
func verifyZip(path, expected string) (err error) {
	archive, err := zip.OpenReader(path)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, archive.Close()) }()
	if len(archive.File) != 1 || archive.File[0].Name != "kitonyterms.exe" || archive.File[0].UncompressedSize64 > update.MaxPackage {
		return errors.New("便携包必须只有kitonyterms.exe")
	}
	file, err := archive.File[0].Open()
	if err != nil {
		return err
	}
	hash := sha256.New()
	count, readErr := io.Copy(hash, io.LimitReader(file, int64(update.MaxPackage)+1))
	err = errors.Join(readErr, file.Close())
	if err != nil || uint64(count) > update.MaxPackage || hex.EncodeToString(hash.Sum(nil)) != expected {
		return errors.Join(errors.New("便携包载荷不一致"), err)
	}
	return nil
}

type boundedOutput struct{ buffer bytes.Buffer }

func (b *boundedOutput) Write(p []byte) (int, error) {
	if b.buffer.Len()+len(p) > 256<<10 {
		return 0, errors.New("原生工具输出超限")
	}
	return b.buffer.Write(p)
}
func execute(ctx context.Context, dir, executable string, args ...string) ([]byte, error) {
	return executeEnv(ctx, nil, dir, executable, args...)
}
func executeEnv(ctx context.Context, environment []string, dir, executable string, args ...string) ([]byte, error) {
	command := exec.CommandContext(ctx, executable, args...)
	command.Env = environment
	command.Dir = dir
	command.WaitDelay = time.Second
	var output, stderr boundedOutput
	command.Stdout, command.Stderr = &output, &stderr
	if err := command.Run(); err != nil {
		return output.buffer.Bytes(), errors.Join(fmt.Errorf("原生工具未成功完成：%w", err), ctx.Err())
	}
	return output.buffer.Bytes(), nil
}
func compile(ctx context.Context, path, iscc string) error {
	if runtime.GOOS != "windows" {
		return errors.New("Inno编译只允许在Windows原生主机执行")
	}
	path, err := absolute(path)
	if err != nil {
		return err
	}
	iscc, err = absolute(iscc)
	if err != nil || !strings.EqualFold(filepath.Base(iscc), "ISCC.exe") {
		return errors.Join(errors.New("必须显式指定ISCC.exe绝对路径"), err)
	}
	plan, err := verifyPrepared(path)
	if err != nil {
		return err
	}
	output := filepath.Join(path, baseName(plan.Identity)+"-setup.exe")
	if _, err = os.Lstat(output); !errors.Is(err, os.ErrNotExist) {
		return errors.Join(errors.New("拒绝覆盖已有安装器"), err)
	}
	probe, cancel := context.WithTimeout(ctx, 5*time.Second)
	value, runErr := execute(probe, path, filepath.Join(path, binaryName), "--update-info")
	cancel()
	actual, identityErr := identity(value)
	if runErr != nil || identityErr != nil || actual != plan.Identity {
		return errors.Join(errors.New("Windows载荷实际发布身份未得到确认"), runErr, identityErr)
	}
	build, cancel := context.WithTimeout(ctx, 5*time.Minute)
	_, err = execute(build, path, iscc, "/Qp", "project.iss")
	cancel()
	if err != nil {
		return err
	}
	if _, err = verifyPrepared(path); err != nil {
		return err
	}
	file, err := pe.Open(output)
	if err != nil {
		return errors.New("ISCC没有生成有效PE安装器")
	}
	if err = file.Close(); err != nil {
		return err
	}
	digest, err := digestFile(output)
	if err != nil {
		return err
	}
	data, err := json.Marshal(struct {
		Identity update.BuildInfo `json:"identity"`
		SHA256   string           `json:"sha256"`
		Signed   bool             `json:"formalSigningVerified"`
	}{plan.Identity, digest, false})
	if err != nil {
		return err
	}
	return writeFile(filepath.Join(path, "compiled.json"), bytes.NewReader(append(data, '\n')), 0600)
}
func run(ctx context.Context, args []string, output io.Writer) error {
	invalid := func() error {
		return errors.New("参数无效：prepare --binary ABS --identity ABS --output NEW_ABS；verify --prepared ABS；compile --prepared ABS --iscc ABS；build --project ABS --identity ABS --output NEW_ABS；verify-resources --project ABS --binary ABS --identity ABS")
	}
	if len(args) == 0 {
		return invalid()
	}
	values := map[string]string{}
	for i := 1; i < len(args); i += 2 {
		if i+1 >= len(args) || args[i+1] == "" || values[args[i]] != "" {
			return invalid()
		}
		values[args[i]] = args[i+1]
	}
	var plan Plan
	var err error
	switch args[0] {
	case "build":
		if len(values) != 3 || values["--project"] == "" || values["--identity"] == "" || values["--output"] == "" {
			return invalid()
		}
		record, err := buildCandidate(ctx, values["--project"], values["--identity"], values["--output"])
		if err != nil {
			return err
		}
		return json.NewEncoder(output).Encode(record)
	case "verify-resources":
		if len(values) != 3 || values["--project"] == "" || values["--binary"] == "" || values["--identity"] == "" {
			return invalid()
		}
		report, err := verifyResources(values["--project"], values["--binary"], values["--identity"])
		if err != nil {
			return err
		}
		return json.NewEncoder(output).Encode(report)
	case "prepare":
		if len(values) != 3 || values["--binary"] == "" || values["--identity"] == "" || values["--output"] == "" {
			return invalid()
		}
		plan, err = prepare(values["--binary"], values["--identity"], values["--output"])
	case "verify":
		if len(values) != 1 || values["--prepared"] == "" {
			return invalid()
		}
		plan, err = verifyPrepared(values["--prepared"])
	case "compile":
		if len(values) != 2 || values["--prepared"] == "" || values["--iscc"] == "" {
			return invalid()
		}
		if err = compile(ctx, values["--prepared"], values["--iscc"]); err == nil {
			plan, err = verifyPrepared(values["--prepared"])
		}
	default:
		return invalid()
	}
	if err != nil {
		return err
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	status := "prepared"
	if args[0] == "compile" {
		status = "compiled_unsigned"
	}
	return json.NewEncoder(output).Encode(struct {
		Status                  string           `json:"status"`
		Identity                update.BuildInfo `json:"identity"`
		RuntimeIdentityVerified bool             `json:"runtimeIdentityVerified"`
	}{status, plan.Identity, args[0] == "compile"})
}
func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	if err := run(ctx, os.Args[1:], os.Stdout); err != nil {
		os.Stderr.WriteString(err.Error() + "\n")
		os.Exit(1)
	}
}
