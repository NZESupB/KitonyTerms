package main

import (
	"bytes"
	"context"
	_ "embed"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"encoding/xml"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"kitonyterms/internal/buildinputs"
	"kitonyterms/internal/update"
)

//go:embed bundle/Info.plist
var plistTemplate string

func candidatePlist(info update.BuildInfo) ([]byte, error) {
	encoded, err := json.Marshal(info)
	if err != nil {
		return nil, err
	}
	if _, err = parseIdentity(encoded); err != nil {
		return nil, err
	}
	var escaped bytes.Buffer
	if err = xml.EscapeText(&escaped, encoded); err != nil {
		return nil, err
	}
	// 三段产品版本与完整公开构建身份分开；不截断CI build/attempt。
	return []byte(strings.NewReplacer("{{VERSION}}", info.Version, "{{IDENTITY}}", escaped.String()).Replace(plistTemplate)), nil
}

func candidateEnvironment() []string {
	return environment(map[string]string{
		"GOOS": "darwin", "GOARCH": runtime.GOARCH, "CGO_ENABLED": "1", "GOWORK": "off", "GOFLAGS": "",
		"CC": "/usr/bin/clang", "CXX": "/usr/bin/clang++", "CGO_CFLAGS": "-mmacosx-version-min=15.0",
		"CGO_CXXFLAGS": "-mmacosx-version-min=15.0", "CGO_LDFLAGS": "-mmacosx-version-min=15.0",
		"MACOSX_DEPLOYMENT_TARGET": "15.0", "DYLD_INSERT_LIBRARIES": "", "DYLD_LIBRARY_PATH": "", "DYLD_FRAMEWORK_PATH": "",
	})
}

func buildCandidate(ctx context.Context, project, output string, info update.BuildInfo) (record Record, err error) {
	if err = ctx.Err(); err != nil {
		return record, err
	}
	mod, err := readBounded(filepath.Join(project, "go.mod"), 64<<10)
	if err != nil || strings.TrimSpace(strings.SplitN(string(mod), "\n", 2)[0]) != "module kitonyterms" {
		return record, errors.Join(errors.New("必须显式指定KitonyTerms app工程"), err)
	}
	if _, err = readBounded(filepath.Join(project, "frontend/dist/index.html"), 1<<20); err != nil {
		return record, errors.Join(errors.New("先完成前端生产构建"), err)
	}
	plist, err := candidatePlist(info)
	if err != nil {
		return record, err
	}
	output, err = newOutput(project, output)
	if err != nil {
		return record, err
	}
	copied := filepath.Join(output, "build-inputs")
	inputs, err := buildinputs.Copy(ctx, project, copied, resources)
	if err != nil {
		return record, err
	}
	icon, err := readBounded(filepath.Join(copied, resources[0]), 8<<20)
	if err != nil || len(icon) < 8 || string(icon[:4]) != "icns" || int(binary.BigEndian.Uint32(icon[4:8])) != len(icon) {
		return record, errors.Join(errors.New("产品ICNS头或体积无效"), err)
	}
	bundle := filepath.Join(output, "payload", bundleName)
	for _, directory := range []string{"Contents/MacOS", "Contents/Resources"} {
		if err = os.MkdirAll(filepath.Join(bundle, directory), 0700); err != nil {
			return record, err
		}
	}
	encoded, err := json.Marshal(info)
	if err != nil {
		return record, err
	}
	inputJSON, err := json.MarshalIndent(inputs, "", "  ")
	if err != nil {
		return record, err
	}
	for name, data := range map[string][]byte{"identity.json": encoded, "build-inputs.json": inputJSON, "payload/" + bundleName + "/" + plistName: plist, "payload/" + bundleName + "/" + iconName: icon} {
		if err = writeFile(filepath.Join(output, name), data, 0600); err != nil {
			return record, err
		}
	}
	goTool, err := exec.LookPath("go")
	if err != nil {
		return record, err
	}
	binary := filepath.Join(bundle, executableName)
	flags := "-w -s -X kitonyterms/internal/update.embeddedIdentity=" + base64.RawURLEncoding.EncodeToString(encoded)
	stdout, stderr, buildErr := runTool(ctx, 5*time.Minute, 256<<10, copied, candidateEnvironment(), goTool,
		"build", "-mod=readonly", "-tags=production", "-trimpath", "-buildvcs=false", "-ldflags="+flags, "-o", binary, ".")
	if err = errors.Join(buildErr, writeFile(filepath.Join(output, "build.log"), append(stdout, stderr...), 0600)); err != nil {
		return record, err
	}
	if err = os.Chmod(binary, 0700); err != nil {
		return record, err
	}
	sdk, err := checkBinary(binary)
	if err != nil {
		return record, err
	}
	if _, _, err = runTool(ctx, 20*time.Second, 4096, output, nil, "/usr/bin/plutil", "-lint", filepath.Join(bundle, plistName)); err != nil {
		return record, err
	}
	if _, _, err = runTool(ctx, 30*time.Second, 4096, output, nil, "/usr/bin/codesign", "--force", "--sign", "-", "--identifier", "com.kitonyterms.app", bundle); err != nil {
		return record, err
	}
	if err = verifyBundle(ctx, output, bundle, info, icon); err != nil {
		return record, err
	}
	bundleFiles, err := bundleHashes(bundle)
	if err != nil {
		return record, err
	}
	if _, _, err = runTool(ctx, 90*time.Second, 64<<10, output, nil, "/usr/bin/hdiutil", "create", "-quiet", "-format", "UDZO", "-fs", "HFS+", "-volname", "KitonyTerms", "-srcfolder", filepath.Join(output, "payload"), filepath.Join(output, dmgName)); err != nil {
		return record, err
	}
	if err = verifyDMG(ctx, output, info, icon, bundleFiles); err != nil {
		return record, err
	}
	for _, directory := range []string{project, copied} {
		if err = buildinputs.Verify(ctx, directory, resources, inputs); err != nil {
			return record, err
		}
	}
	record = Record{Schema: 1, Status: "candidate_local_verified", Identity: info, Inputs: inputs, Files: map[string]string{}, Signing: "local_adhoc", MinimumSystem: "15.0.0", MachOSDK: sdk, RuntimeIdentityVerified: true, DataCreationVerified: true, DMGReadbackVerified: true}
	for name, hash := range bundleFiles {
		record.Files["payload/"+bundleName+"/"+name] = hash
	}
	for _, name := range []string{"identity.json", "build-inputs.json", "build.log", dmgName} {
		record.Files[name], err = digestFile(filepath.Join(output, name))
		if err != nil {
			return record, err
		}
	}
	if err = ctx.Err(); err != nil {
		return record, err
	}
	data, err := json.MarshalIndent(record, "", "  ")
	if err != nil {
		return record, err
	}
	if err = writeFile(filepath.Join(output, recordName), append(data, '\n'), 0600); err != nil {
		return record, err
	}
	return record, nil
}
