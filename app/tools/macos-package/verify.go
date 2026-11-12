package main

import (
	"bytes"
	"context"
	"debug/buildinfo"
	"debug/macho"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"time"

	"kitonyterms/internal/buildinputs"
	"kitonyterms/internal/update"
)

func versionText(value uint32) string {
	return fmt.Sprintf("%d.%d.%d", value>>16, value>>8&255, value&255)
}

func checkBinary(path string) (sdk string, err error) {
	data, err := readBounded(path, 128<<20)
	if err != nil {
		return "", err
	}
	if err = validateMachOBounds(data); err != nil {
		return "", err
	}
	image, err := macho.NewFile(bytes.NewReader(data))
	if err != nil {
		return "", errors.New("候选程序不是单架构Mach-O")
	}
	defer image.Close()
	if (image.Cpu != macho.CpuArm64 && image.Cpu != macho.CpuAmd64) || image.Type != macho.TypeExec {
		return "", errors.New("候选程序必须是ARM64/amd64 Mach-O可执行文件")
	}
	declarations := 0
	for _, load := range image.Loads {
		raw := load.Raw()
		if len(raw) < 8 {
			return "", errors.New("Mach-O加载声明不完整")
		}
		command := image.ByteOrder.Uint32(raw[:4])
		var minimum, builtSDK uint32
		switch command {
		case 0x32: // LC_BUILD_VERSION
			if len(raw) < 24 || image.ByteOrder.Uint32(raw[8:12]) != 1 {
				return "", errors.New("Mach-O构建平台必须为macOS")
			}
			minimum, builtSDK = image.ByteOrder.Uint32(raw[12:16]), image.ByteOrder.Uint32(raw[16:20])
		case 0x24: // LC_VERSION_MIN_MACOSX
			if len(raw) < 16 {
				return "", errors.New("Mach-O最低系统声明不完整")
			}
			minimum, builtSDK = image.ByteOrder.Uint32(raw[8:12]), image.ByteOrder.Uint32(raw[12:16])
		default:
			continue
		}
		declarations++
		if minimum != 15<<16 || builtSDK == 0 {
			return "", errors.New("Mach-O最低系统必须明确为15.0且包含SDK声明")
		}
		sdk = versionText(builtSDK)
	}
	if declarations != 1 {
		return "", errors.New("Mach-O必须包含唯一最低系统声明")
	}
	build, err := buildinfo.Read(bytes.NewReader(data))
	if err != nil || build.Path != "kitonyterms" || build.Main.Path != "kitonyterms" {
		return "", errors.New("缺少KitonyTerms Go构建信息")
	}
	settings := map[string]string{}
	for _, setting := range build.Settings {
		settings[setting.Key] = setting.Value
	}
	expectedArch := map[macho.Cpu]string{macho.CpuArm64: "arm64", macho.CpuAmd64: "amd64"}[image.Cpu]
	if settings["GOOS"] != "darwin" || settings["GOARCH"] != expectedArch || settings["CGO_ENABLED"] != "1" || settings["-trimpath"] != "true" {
		return "", errors.New("Go平台/cgo/trimpath构建信息不匹配")
	}
	production := false
	for _, tag := range strings.Split(settings["-tags"], ",") {
		if tag == "server" {
			return "", errors.New("server构建不能作为桌面候选")
		}
		production = production || tag == "production"
	}
	pinned := false
	for _, dependency := range build.Deps {
		if dependency.Path == "github.com/wailsapp/wails/v3" {
			pinned = dependency.Version == "v3.1.0" && dependency.Replace == nil
		}
	}
	if !production || !pinned {
		return "", errors.New("候选必须为production及无替换的Wails v3.1.0")
	}
	return sdk, nil
}

func bundleHashes(bundle string) (files map[string]string, err error) {
	before, err := os.Lstat(bundle)
	if err != nil || !before.IsDir() || before.Mode()&os.ModeSymlink != 0 {
		return nil, errors.Join(errors.New("候选应用必须是实际目录"), err)
	}
	root, err := os.OpenRoot(bundle)
	if err != nil {
		return nil, err
	}
	defer func() { err = errors.Join(err, root.Close()) }()
	files = map[string]string{plistName: "", iconName: "", executableName: "", "Contents/_CodeSignature/CodeResources": ""}
	directories := map[string]bool{".": true, "Contents": true, "Contents/MacOS": true, "Contents/Resources": true, "Contents/_CodeSignature": true}
	for name := range directories {
		info, statErr := root.Lstat(name)
		if statErr != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			return nil, errors.Join(errors.New("候选应用缺少实际目录或含链接"), statErr)
		}
		directory, openErr := root.Open(name)
		if openErr != nil {
			return nil, openErr
		}
		opened, statErr := directory.Stat()
		if statErr != nil || !os.SameFile(info, opened) {
			return nil, errors.Join(errors.New("候选目录在打开时变化"), statErr, directory.Close())
		}
		entries, readErr := directory.ReadDir(16)
		after, statErr := root.Lstat(name)
		closeErr := directory.Close()
		if readErr != nil && readErr != io.EOF {
			return nil, errors.Join(readErr, statErr, closeErr)
		}
		if statErr != nil || closeErr != nil || !os.SameFile(info, after) || !info.ModTime().Equal(after.ModTime()) || len(entries) >= 16 {
			return nil, errors.Join(errors.New("候选目录变化或项目数超限"), statErr, closeErr)
		}
		for _, entry := range entries {
			child := filepath.ToSlash(filepath.Join(name, entry.Name()))
			if entry.Type()&os.ModeSymlink != 0 {
				return nil, errors.New("候选应用包不允许链接")
			}
			if entry.IsDir() {
				if !directories[child] {
					return nil, errors.New("候选应用包含未声明目录")
				}
			} else if _, allowed := files[child]; !allowed {
				return nil, errors.New("候选应用包含未声明文件")
			}
		}
	}
	for name := range files {
		files[name], err = digestFile(filepath.Join(bundle, name))
		if err != nil {
			return nil, err
		}
	}
	after, err := os.Lstat(bundle)
	if err != nil || !os.SameFile(before, after) {
		return nil, errors.Join(errors.New("候选应用根目录身份变化"), err)
	}
	return files, nil
}

func verifyRuntime(ctx context.Context, parent, binary string, info update.BuildInfo) (err error) {
	probe, err := os.MkdirTemp(parent, ".identity-probe-")
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, os.Remove(probe)) }()
	fixed := map[string]string{"KITONYTERMS_DATA_DIR": filepath.Join(probe, "must-not-be-created"), "DYLD_INSERT_LIBRARIES": "", "DYLD_LIBRARY_PATH": "", "DYLD_FRAMEWORK_PATH": ""}
	stdout, _, runErr := runTool(ctx, 10*time.Second, 4096, probe, environment(fixed), binary, "--update-info")
	entries, readErr := os.ReadDir(probe)
	if runErr != nil || readErr != nil || len(entries) != 0 {
		return errors.Join(errors.New("只读身份查询失败或创建了业务数据，材料保留"), runErr, readErr)
	}
	actual, err := parseIdentity(stdout)
	if err != nil || actual != info {
		return errors.Join(errors.New("候选实际运行身份与公开BuildInfo不符"), err)
	}
	return ctx.Err()
}

func verifyBundle(ctx context.Context, parent, bundle string, info update.BuildInfo, icon []byte) error {
	if _, err := bundleHashes(bundle); err != nil {
		return err
	}
	plist, err := candidatePlist(info)
	if err != nil {
		return err
	}
	for name, expected := range map[string][]byte{plistName: plist, iconName: icon} {
		actual, readErr := readBounded(filepath.Join(bundle, name), 8<<20)
		if readErr != nil || !bytes.Equal(actual, expected) {
			return errors.Join(errors.New("候选产品plist/图标与公开输入不符"), readErr)
		}
	}
	binary := filepath.Join(bundle, executableName)
	if _, err = checkBinary(binary); err != nil {
		return err
	}
	if _, _, err = runTool(ctx, 30*time.Second, 4096, parent, nil, "/usr/bin/codesign", "--verify", "--deep", "--strict", bundle); err != nil {
		return err
	}
	_, display, err := runTool(ctx, 10*time.Second, 4096, parent, nil, "/usr/bin/codesign", "--display", "--verbose=4", bundle)
	if err != nil || !bytes.Contains(display, []byte("\nSignature=adhoc\n")) || !bytes.Contains(display, []byte("\nIdentifier=com.kitonyterms.app\n")) || bytes.Contains(display, []byte("\nAuthority=")) {
		return errors.Join(errors.New("候选签名必须是明确的本地ad-hoc应用身份"), err)
	}
	return verifyRuntime(ctx, parent, binary, info)
}

func detachDMG(mount string) error {
	_, _, err := runTool(context.Background(), 20*time.Second, 64<<10, "", nil, "/usr/bin/hdiutil", "detach", mount)
	if err == nil {
		return nil
	}
	_, _, forced := runTool(context.Background(), 20*time.Second, 64<<10, "", nil, "/usr/bin/hdiutil", "detach", "-force", mount)
	if forced != nil {
		return errors.Join(err, forced)
	}
	return nil
}

// 仅清理本次独占创建的挂载点；取消后也等待卸载真实结束。
func withDMG(ctx context.Context, output string, consume func(string) error) (err error) {
	mount := filepath.Join(output, ".dmg-mount")
	if err = os.Mkdir(mount, 0700); err != nil {
		return err
	}
	defer func() {
		active, _, checkErr := mountedReadOnly(mount, output)
		if checkErr != nil {
			err = errors.Join(err, checkErr)
			return
		}
		if active {
			if cleanup := detachDMG(mount); cleanup != nil {
				err = errors.Join(err, fmt.Errorf("候选DMG卸载失败，挂载点保留：%w", cleanup))
				return
			}
		}
		err = errors.Join(err, os.Remove(mount))
	}()
	image := filepath.Join(output, dmgName)
	if _, _, err = runTool(ctx, 90*time.Second, 64<<10, output, nil, "/usr/bin/hdiutil", "verify", "-quiet", image); err != nil {
		return err
	}
	if _, _, err = runTool(ctx, 90*time.Second, 64<<10, output, nil, "/usr/bin/hdiutil", "attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint", mount, image); err != nil {
		return err
	}
	active, readonly, err := mountedReadOnly(mount, output)
	if err != nil || !active || !readonly {
		return errors.Join(errors.New("候选镜像未实际只读挂载"), err)
	}
	if err = consume(mount); err != nil {
		return err
	}
	return ctx.Err()
}

func verifyDMG(ctx context.Context, output string, info update.BuildInfo, icon []byte, expected map[string]string) error {
	before, err := digestFile(filepath.Join(output, dmgName))
	if err != nil {
		return err
	}
	if err = withDMG(ctx, output, func(mount string) error {
		bundle := filepath.Join(mount, bundleName)
		actual, err := bundleHashes(bundle)
		if err != nil || !reflect.DeepEqual(expected, actual) {
			return errors.Join(errors.New("DMG包内字节与已验证应用不符"), err)
		}
		return verifyBundle(ctx, output, bundle, info, icon)
	}); err != nil {
		return err
	}
	after, err := digestFile(filepath.Join(output, dmgName))
	if err != nil || after != before {
		return errors.Join(errors.New("校验期间DMG变化"), err)
	}
	return nil
}

func verifyCandidate(ctx context.Context, project, output string, info update.BuildInfo) (record Record, err error) {
	if err = ctx.Err(); err != nil {
		return record, err
	}
	if buildinputs.Overlap(project, output) {
		return record, errors.New("候选目录不能与原工程重叠")
	}
	data, err := readBounded(filepath.Join(output, recordName), 1<<20)
	if err != nil {
		return record, err
	}
	if err = decodeJSON(data, &record); err != nil {
		return record, err
	}
	if record.Schema != 1 || record.Status != "candidate_local_verified" || record.Identity != info || record.Signing != "local_adhoc" || record.MinimumSystem != "15.0.0" || !record.RuntimeIdentityVerified || !record.DataCreationVerified || !record.DMGReadbackVerified || record.MinimumOSExecuted || record.FormalSigningVerified {
		return record, errors.New("候选完成记录身份或验证边界不符")
	}
	bundle := filepath.Join(output, "payload", bundleName)
	payload, err := os.Lstat(filepath.Join(output, "payload"))
	if err != nil || !payload.IsDir() || payload.Mode()&os.ModeSymlink != 0 {
		return record, errors.Join(errors.New("候选payload必须为实际目录"), err)
	}
	bundleFiles, err := bundleHashes(bundle)
	if err != nil {
		return record, err
	}
	expectedFiles := map[string]bool{"identity.json": true, "build-inputs.json": true, "build.log": true, dmgName: true}
	for name := range bundleFiles {
		expectedFiles["payload/"+bundleName+"/"+name] = true
	}
	if len(record.Files) != len(expectedFiles) {
		return record, errors.New("候选产物清单集合不符")
	}
	for name := range expectedFiles {
		hash, hashErr := digestFile(filepath.Join(output, name))
		if hashErr != nil || record.Files[name] != hash {
			return record, errors.Join(errors.New("候选产物摘要不符"), hashErr)
		}
	}
	identityJSON, err := readBounded(filepath.Join(output, "identity.json"), 4096)
	if err != nil {
		return record, err
	}
	declared, err := parseIdentity(identityJSON)
	if err != nil || declared != info {
		return record, errors.Join(errors.New("候选公开身份文件不符"), err)
	}
	inputJSON, err := readBounded(filepath.Join(output, "build-inputs.json"), 1<<20)
	if err != nil {
		return record, err
	}
	var declaredInputs map[string]string
	if err = decodeJSON(inputJSON, &declaredInputs); err != nil || !reflect.DeepEqual(declaredInputs, record.Inputs) {
		return record, errors.Join(errors.New("候选输入清单不符"), err)
	}
	for _, directory := range []string{project, filepath.Join(output, "build-inputs")} {
		if err = buildinputs.Verify(ctx, directory, resources, record.Inputs); err != nil {
			return record, err
		}
	}
	sdk, err := checkBinary(filepath.Join(bundle, executableName))
	if err != nil || sdk != record.MachOSDK {
		return record, errors.Join(errors.New("候选Mach-O声明不符"), err)
	}
	icon, err := readBounded(filepath.Join(project, resources[0]), 8<<20)
	if err != nil {
		return record, err
	}
	if err = verifyBundle(ctx, output, bundle, info, icon); err != nil {
		return record, err
	}
	return record, verifyDMG(ctx, output, info, icon, bundleFiles)
}
