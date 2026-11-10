// 独立macOS候选工具：仅构建新私有材料和本地测试签名，不安装或发布。
package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"

	"kitonyterms/internal/buildinputs"
	"kitonyterms/internal/update"
)

const bundleName = "KitonyTerms.app"
const executableName = "Contents/MacOS/kitonyterms"
const iconName = "Contents/Resources/candidate.icns"
const plistName = "Contents/Info.plist"
const dmgName = "kitonyterms-candidate-macos-arm64.dmg"
const recordName = "candidate-build.json"

var resources = []string{"build/darwin/candidate.icns"}

type Record struct {
	Schema                  int               `json:"schema"`
	Status                  string            `json:"status"`
	Identity                update.BuildInfo  `json:"identity"`
	Inputs                  map[string]string `json:"inputs"`
	Files                   map[string]string `json:"files"`
	Signing                 string            `json:"signing"`
	MinimumSystem           string            `json:"minimumSystem"`
	MachOSDK                string            `json:"machOSDK"`
	RuntimeIdentityVerified bool              `json:"runtimeIdentityVerified"`
	DataCreationVerified    bool              `json:"dataCreationVerified"`
	DMGReadbackVerified     bool              `json:"dmgReadbackVerified"`
	MinimumOSExecuted       bool              `json:"minimumOSExecuted"`
	FormalSigningVerified   bool              `json:"formalSigningVerified"`
}

func decodeJSON(data []byte, target any) error {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(target); err != nil {
		return errors.New("候选身份或清单JSON无效")
	}
	var extra any
	if decoder.Decode(&extra) != io.EOF {
		return errors.New("候选身份或清单含多余内容")
	}
	return nil
}

func parseIdentity(data []byte) (info update.BuildInfo, err error) {
	if len(data) > 4096 {
		return info, errors.New("公开构建身份超限")
	}
	if err = decodeJSON(data, &info); err != nil {
		return info, err
	}
	if err = info.Validate(); err != nil {
		return info, err
	}
	if info.Target != "aarch64-apple-darwin" && info.Target != "x86_64-apple-darwin" {
		return info, errors.New("此候选入口仅支持macOS ARM64/amd64")
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

func newOutput(project, output string) (string, error) {
	if !filepath.IsAbs(output) {
		return "", errors.New("候选目录必须为新绝对路径")
	}
	parent, err := absolute(filepath.Dir(filepath.Clean(output)))
	if err != nil {
		return "", err
	}
	output = filepath.Join(parent, filepath.Base(filepath.Clean(output)))
	if buildinputs.Overlap(project, output) {
		return "", errors.New("候选目录不能与原工程重叠")
	}
	if err = os.Mkdir(output, 0700); err != nil {
		return "", err
	}
	return output, nil
}

func run(ctx context.Context, args []string, stdout io.Writer) error {
	if len(args) == 0 || args[0] != "build" && args[0] != "verify" {
		return errors.New("用法：macos-package build --project ABS --identity ABS --output NEW_ABS；verify --project ABS --identity ABS --candidate ABS")
	}
	flags := flag.NewFlagSet(args[0], flag.ContinueOnError)
	flags.SetOutput(io.Discard)
	project := flags.String("project", "", "app绝对目录")
	identityPath := flags.String("identity", "", "公开BuildInfo绝对文件")
	var output *string
	if args[0] == "build" {
		output = flags.String("output", "", "新候选绝对目录")
	} else {
		output = flags.String("candidate", "", "已有候选绝对目录")
	}
	if err := flags.Parse(args[1:]); err != nil || flags.NArg() != 0 || *project == "" || *identityPath == "" || *output == "" {
		return errors.New("候选参数无效或缺少必填绝对路径")
	}
	if runtime.GOOS != "darwin" || runtime.GOARCH != "arm64" && runtime.GOARCH != "amd64" {
		return errors.New("macOS候选构建与原生验证需要Darwin主机")
	}
	canonical, err := absolute(*project)
	if err != nil {
		return err
	}
	identityFile, err := absolute(*identityPath)
	if err != nil {
		return err
	}
	data, err := readBounded(identityFile, 4096)
	if err != nil {
		return err
	}
	info, err := parseIdentity(data)
	if err != nil {
		return err
	}
	if info.Target != map[string]string{"arm64": "aarch64-apple-darwin", "amd64": "x86_64-apple-darwin"}[runtime.GOARCH] {
		return errors.New("候选身份必须与原生主机架构匹配")
	}
	var record Record
	if args[0] == "build" {
		record, err = buildCandidate(ctx, canonical, *output, info)
	} else {
		var candidate string
		candidate, err = absolute(*output)
		if err == nil {
			record, err = verifyCandidate(ctx, canonical, candidate, info)
		}
	}
	if err != nil {
		return fmt.Errorf("候选操作未完成，已有材料保留：%w", err)
	}
	return json.NewEncoder(stdout).Encode(record)
}

func main() {
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt)
	defer cancel()
	if err := run(ctx, os.Args[1:], os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
