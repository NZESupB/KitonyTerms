package update

import (
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"runtime"
	"strconv"
	"strings"

	"kitonyterms/internal/model"
)

const MaxPackage uint64 = 512 << 20
const metadataLimit = 2 << 20
const notesLimit = 64 << 10

// 仅在构建时注入公开发布身份，运行时环境不能开启更新或替换更新源。
var embeddedIdentity string

type BuildInfo struct {
	Channel string `json:"channel"`
	Version string `json:"version"`
	Commit  string `json:"commit"`
	Build   uint64 `json:"build"`
	Attempt uint64 `json:"attempt"`
	Target  string `json:"target"`
}

type Package struct {
	Name   string `json:"name"`
	Size   uint64 `json:"size"`
	SHA256 string `json:"sha256"`
	Kind   string `json:"kind"`
}

type targetPackages struct {
	Archive   Package  `json:"archive"`
	Installer *Package `json:"installer"`
}
type Manifest struct {
	Schema  int                       `json:"schema"`
	Channel string                    `json:"channel"`
	Version string                    `json:"version"`
	Commit  string                    `json:"commit"`
	Build   uint64                    `json:"build"`
	Attempt uint64                    `json:"attempt"`
	Targets map[string]targetPackages `json:"targets"`
}

type available struct {
	Info                    BuildInfo
	Package                 Package
	AssetID                 uint64
	ReleaseID               uint64
	Tag, Notes, PublishedAt string
	NotesTruncated          bool
}

func desktopTarget() string {
	switch runtime.GOOS + "/" + runtime.GOARCH {
	case "darwin/arm64":
		return "aarch64-apple-darwin"
	case "darwin/amd64":
		return "x86_64-apple-darwin"
	case "windows/amd64":
		return "x86_64-pc-windows-msvc"
	case "windows/arm64":
		return "aarch64-pc-windows-msvc"
	case "linux/amd64":
		return "x86_64-unknown-linux-gnu"
	case "linux/arm64":
		return "aarch64-unknown-linux-gnu"
	default:
		return ""
	}
}
func CurrentBuild() (BuildInfo, error) {
	if embeddedIdentity == "" {
		return BuildInfo{Channel: "development", Version: "0.2.0", Target: desktopTarget()}, nil
	}
	data, err := base64.RawURLEncoding.DecodeString(embeddedIdentity)
	if err != nil || len(data) > 4096 {
		return BuildInfo{}, fmt.Errorf("构建发布身份编码无效")
	}
	var info BuildInfo
	if err = decodeJSON(data, &info, true); err != nil {
		return BuildInfo{}, fmt.Errorf("构建发布身份无效")
	}
	if err = info.Validate(); err != nil {
		return BuildInfo{}, err
	}
	if info.Target != desktopTarget() {
		return BuildInfo{}, fmt.Errorf("构建发布目标与当前平台不匹配")
	}
	return info, nil
}
func validHex(value string, size int) bool {
	_, err := hex.DecodeString(value)
	return len(value) == size && err == nil
}
func version(value string) ([3]uint64, error) {
	var result [3]uint64
	parts := strings.Split(value, ".")
	if len(value) > 64 || len(parts) != 3 {
		return result, fmt.Errorf("发布版本无效")
	}
	for i, part := range parts {
		if part == "" || len(part) > 1 && part[0] == '0' {
			return result, fmt.Errorf("发布版本无效")
		}
		for _, c := range part {
			if c < '0' || c > '9' {
				return result, fmt.Errorf("发布版本无效")
			}
		}
		n, err := strconv.ParseUint(part, 10, 64)
		if err != nil {
			return result, fmt.Errorf("发布版本超出范围")
		}
		result[i] = n
	}
	return result, nil
}
func (b BuildInfo) Validate() error {
	if b.Channel != "stable" && b.Channel != "alpha" {
		return fmt.Errorf("发布渠道无效")
	}
	if _, err := version(b.Version); err != nil {
		return err
	}
	if !validHex(b.Commit, 40) || b.Build == 0 || b.Attempt == 0 {
		return fmt.Errorf("发布身份缺失")
	}
	switch b.Target {
	case "aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu":
	default:
		return fmt.Errorf("更新发布目标无效")
	}
	return nil
}
func (b BuildInfo) dto() model.UpdateBuild {
	return model.UpdateBuild{Channel: b.Channel, Version: b.Version, Commit: b.Commit, Build: strconv.FormatUint(b.Build, 10), Attempt: strconv.FormatUint(b.Attempt, 10), Target: b.Target}
}
func (p Package) Validate() error {
	if p.Size == 0 || p.Size > MaxPackage || !validHex(p.SHA256, 64) || !strings.HasPrefix(p.Name, "kitonyterms-") || len(p.Name) > 256 || strings.Contains(p.Name, "..") {
		return fmt.Errorf("更新包身份无效")
	}
	for _, c := range p.Name {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '.' || c == '_' || c == '-') {
			return fmt.Errorf("更新包名称无效")
		}
	}
	return nil
}
func (m Manifest) newer(current BuildInfo, tag string) (bool, error) {
	if err := current.Validate(); err != nil {
		return false, err
	}
	if m.Schema != 1 || m.Channel != current.Channel || !validHex(m.Commit, 40) || m.Build == 0 || m.Attempt == 0 {
		return false, fmt.Errorf("更新协议或渠道身份不匹配")
	}
	next, err := version(m.Version)
	if err != nil {
		return false, err
	}
	if m.Channel == "alpha" {
		if tag != "alpha" {
			return false, fmt.Errorf("Alpha标签不匹配")
		}
		return m.Build > current.Build || m.Build == current.Build && m.Attempt > current.Attempt, nil
	}
	if tag != "v"+m.Version {
		return false, fmt.Errorf("正式发布标签不匹配")
	}
	previous, _ := version(current.Version)
	for i := range next {
		if next[i] != previous[i] {
			return next[i] > previous[i], nil
		}
	}
	return false, nil
}
func (m Manifest) packageFor(target string, installed bool) (Package, error) {
	entry, ok := m.Targets[target]
	if !ok {
		return Package{}, fmt.Errorf("缺少当前平台更新包")
	}
	p := entry.Archive
	kind := "tar_gz"
	suffix := ".tar.gz"
	if strings.Contains(target, "apple") {
		kind = "dmg"
		suffix = ".dmg"
	} else if strings.Contains(target, "windows") {
		kind = "zip"
		suffix = ".zip"
	}
	if installed {
		if !strings.Contains(target, "windows") || entry.Installer == nil {
			return Package{}, fmt.Errorf("缺少当前平台安装器")
		}
		p = *entry.Installer
		kind = "installer"
		suffix = "-setup.exe"
	}
	if p.Kind != kind || !strings.HasSuffix(p.Name, suffix) {
		return Package{}, fmt.Errorf("更新包类型不匹配")
	}
	if err := p.Validate(); err != nil {
		return Package{}, err
	}
	p.SHA256 = strings.ToLower(p.SHA256)
	return p, nil
}
func (a available) dto() model.UpdateAvailable {
	return model.UpdateAvailable{Info: a.Info.dto(), Package: model.UpdatePackage{Name: a.Package.Name, Size: strconv.FormatUint(a.Package.Size, 10), SHA256: a.Package.SHA256, Kind: a.Package.Kind}, AssetID: strconv.FormatUint(a.AssetID, 10), ReleaseID: strconv.FormatUint(a.ReleaseID, 10), Tag: a.Tag, Notes: a.Notes, NotesTruncated: a.NotesTruncated, PublishedAt: a.PublishedAt}
}
