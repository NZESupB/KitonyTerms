package update

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

// 字段与 Rust 安装记录保持一致；非法或未完成记录不自动覆盖。
type installation struct {
	Executable string `json:"executable"`
	Target     string `json:"target"`
	Bundle     bool   `json:"bundle"`
	Installed  bool   `json:"installed"`
}

type installTask struct {
	Installation installation `json:"installation"`
	Current      BuildInfo    `json:"current"`
	Next         BuildInfo    `json:"next"`
	SourceSHA256 string       `json:"source_sha256"`
	Package      Package      `json:"package"`
	ParentPID    uint32       `json:"parent_pid"`
	Elevate      bool         `json:"elevate"`
}

type installJournal struct {
	Task         installTask `json:"task"`
	Staging      string      `json:"staging"`
	StagedSHA256 string      `json:"staged_sha256"`
	Stage        string      `json:"stage"`
	Storage      string      `json:"storage"`
}

func pendingInstallation(root *os.Root) error {
	for _, name := range []string{"task.json", "journal.json"} {
		if _, err := root.Lstat(name); err == nil {
			return fmt.Errorf("存在未完成的更新事务，请先完成恢复")
		} else if !errors.Is(err, fs.ErrNotExist) {
			return err
		}
	}
	return nil
}

func cleanInstallPath(value string) bool {
	if value == "" || len(value) > 32760 || !filepath.IsAbs(value) || filepath.Clean(value) != value {
		return false
	}
	for _, c := range value {
		if c == 0 || c < 32 {
			return false
		}
	}
	return filepath.Base(value) != "." && filepath.Base(value) != string(filepath.Separator)
}

func (task installTask) validate() error {
	if err := task.Current.Validate(); err != nil {
		return err
	}
	if err := task.Next.Validate(); err != nil {
		return err
	}
	if task.Current.Target != desktopTarget() || task.Current.Target != task.Next.Target || task.Current.Channel != task.Next.Channel || task.ParentPID == 0 || task.ParentPID > 0x7fffffff || !validHex(task.SourceSHA256, 64) {
		return fmt.Errorf("安装任务构建身份无效")
	}
	manifest := Manifest{Schema: 1, Channel: task.Next.Channel, Version: task.Next.Version, Commit: task.Next.Commit, Build: task.Next.Build, Attempt: task.Next.Attempt, Targets: map[string]targetPackages{}}
	tag := "alpha"
	if task.Next.Channel == "stable" {
		tag = "v" + task.Next.Version
	}
	newer, err := manifest.newer(task.Current, tag)
	if err != nil || !newer {
		return fmt.Errorf("安装任务目标版本没有更新")
	}
	install := task.Installation
	if !cleanInstallPath(install.Executable) || !cleanInstallPath(install.Target) {
		return fmt.Errorf("安装目标路径无效")
	}
	windows := strings.Contains(task.Current.Target, "windows")
	mac := strings.Contains(task.Current.Target, "apple-darwin")
	if install.Installed && (!windows || task.Elevate) || task.Elevate && !windows || install.Bundle && (!mac || install.Installed) {
		return fmt.Errorf("安装方式与平台不匹配")
	}
	if install.Bundle {
		if filepath.Base(install.Target) != "KitonyTerms.app" || install.Executable != filepath.Join(install.Target, "Contents", "MacOS", "kitonyterms") {
			return fmt.Errorf("安装应用包路径身份无效")
		}
	} else if install.Executable != install.Target {
		return fmt.Errorf("安装目标与程序路径不匹配")
	}
	if install.Installed {
		manifest.Targets[task.Current.Target] = targetPackages{Installer: &task.Package}
	} else {
		manifest.Targets[task.Current.Target] = targetPackages{Archive: task.Package}
	}
	_, err = manifest.packageFor(task.Current.Target, install.Installed)
	return err
}

func (journal installJournal) validate(root *os.Root) error {
	if err := journal.Task.validate(); err != nil {
		return err
	}
	if journal.Stage != "prepared" && journal.Stage != "backed_up" && journal.Stage != "committed" {
		return fmt.Errorf("安装事务阶段无效")
	}
	if journal.Storage != "legacy" && journal.Storage != "user_directory" {
		return fmt.Errorf("安装事务存储布局无效")
	}
	if journal.StagedSHA256 != "" && !validHex(journal.StagedSHA256, 64) {
		return fmt.Errorf("安装暂存摘要无效")
	}
	if journal.Task.Elevate && journal.StagedSHA256 == "" {
		return fmt.Errorf("提权事务缺少暂存摘要")
	}
	if !cleanInstallPath(journal.Staging) {
		return fmt.Errorf("安装暂存路径无效")
	}
	name := filepath.Base(journal.Staging)
	if !strings.HasPrefix(name, ".kt-update-stage-") || len(name) <= len(".kt-update-stage-") || len(name) > 128 {
		return fmt.Errorf("安装暂存目录名称无效")
	}
	for _, c := range strings.TrimPrefix(name, ".kt-update-stage-") {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-' || c == '_') {
			return fmt.Errorf("安装暂存目录名称无效")
		}
	}
	parent := filepath.Dir(journal.Task.Installation.Target)
	if journal.Storage == "user_directory" || journal.Task.Installation.Installed {
		parent = root.Name()
	}
	expected, err := canonicalUpdatePath(parent)
	if err != nil {
		return err
	}
	actual, err := canonicalUpdatePath(filepath.Dir(journal.Staging))
	if err != nil {
		return err
	}
	if actual != expected {
		return fmt.Errorf("安装暂存目录不属于当前事务")
	}
	if journal.Staging == journal.Task.Installation.Target || journal.Staging == journal.Task.Installation.Executable {
		return fmt.Errorf("安装暂存目录与目标重叠")
	}
	if info, err := os.Lstat(journal.Staging); err == nil && (!info.IsDir() || info.Mode()&fs.ModeSymlink != 0) {
		return fmt.Errorf("安装暂存路径不是普通目录")
	} else if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	return nil
}

func readInstallTask(root *os.Root) (installTask, error) {
	var task installTask
	data, err := readPrivate(root, "task.json", metadataLimit)
	if err != nil {
		return task, err
	}
	if err = decodeJSON(data, &task, true); err != nil {
		return task, fmt.Errorf("安装任务JSON无效：%w", err)
	}
	if err = checkTaskRecord(data); err != nil {
		return task, err
	}
	return task, task.validate()
}

func readInstallJournal(root *os.Root) (installJournal, string, error) {
	var journal installJournal
	data, err := readPrivate(root, "journal.json", metadataLimit)
	if err != nil {
		return journal, "", err
	}
	if err = decodeJSON(data, &journal, true); err != nil {
		return journal, "", fmt.Errorf("安装记录JSON无效：%w", err)
	}
	var fields map[string]json.RawMessage
	if err = json.Unmarshal(data, &fields); err != nil {
		return journal, "", err
	}
	if err = checkTaskRecord(fields["task"]); err != nil {
		return journal, "", err
	}
	if _, present := fields["storage"]; !present {
		journal.Storage = "legacy"
	}
	if value, present := fields["staged_sha256"]; present && string(value) == "null" {
		return journal, "", fmt.Errorf("安装暂存摘要不能为 null")
	}
	if err = journal.validate(root); err != nil {
		return journal, "", err
	}
	digest := sha256.Sum256(data)
	return journal, hex.EncodeToString(digest[:]), nil
}

func checkTaskRecord(data []byte) error {
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return fmt.Errorf("安装任务字段无效：%w", err)
	}
	for _, name := range []string{"installation", "current", "next", "source_sha256", "package", "parent_pid"} {
		if value, ok := fields[name]; !ok || string(value) == "null" {
			return fmt.Errorf("安装任务缺少字段：%s", name)
		}
	}
	if value, ok := fields["elevate"]; ok && string(value) != "true" && string(value) != "false" {
		return fmt.Errorf("安装任务提权字段类型无效")
	}
	var install map[string]json.RawMessage
	if err := json.Unmarshal(fields["installation"], &install); err != nil {
		return fmt.Errorf("安装目标字段无效：%w", err)
	}
	for _, name := range []string{"bundle", "installed"} {
		if value := string(install[name]); value != "true" && value != "false" {
			return fmt.Errorf("安装目标布尔字段无效：%s", name)
		}
	}
	return nil
}

// 旧Rust注册版记录的staged_sha256属于旧EXE；新程序仍须精确--update-info验证。
func (journal installJournal) verificationDigest() string {
	if journal.Task.Installation.Installed && strings.EqualFold(journal.StagedSHA256, journal.Task.SourceSHA256) {
		return ""
	}
	return journal.StagedSHA256
}
