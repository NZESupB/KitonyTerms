package externaledit

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"unicode"

	"kitonyterms/internal/model"
)

// 先解析模板，再替换文件参数；文件名不会成为shell代码。
func Command(template, file string) (string, []string, error) {
	if len(template) > 16384 || strings.ContainsRune(template, 0) {
		return "", nil, fmt.Errorf("编辑器命令无效")
	}
	var tokens []string
	var token strings.Builder
	var quote rune
	escape, started := false, false
	for _, r := range template {
		if escape {
			token.WriteRune(r)
			escape = false
			started = true
			continue
		}
		switch {
		case r == '\\' && quote != '\'':
			escape = true
			started = true
		case r == '\'' && quote != '"', r == '"' && quote != '\'':
			if quote == r {
				quote = 0
			} else {
				quote = r
			}
			started = true
		case unicode.IsSpace(r) && quote == 0:
			if started {
				tokens = append(tokens, token.String())
				token.Reset()
				started = false
			}
		default:
			token.WriteRune(r)
			started = true
		}
	}
	if escape || quote != 0 {
		return "", nil, fmt.Errorf("编辑器命令引号或转义未完成")
	}
	if started {
		tokens = append(tokens, token.String())
	}
	if len(tokens) == 0 || tokens[0] == "" {
		return "", nil, fmt.Errorf("编辑器命令为空")
	}
	program := tokens[0]
	if strings.Contains(program, "{file}") {
		return "", nil, fmt.Errorf("文件占位符不能作为编辑器程序")
	}
	args := tokens[1:]
	replaced := false
	for i, value := range args {
		if strings.Contains(value, "{file}") {
			args[i] = strings.ReplaceAll(value, "{file}", file)
			replaced = true
		}
	}
	if !replaced {
		args = append(args, file)
	}
	return program, args, nil
}
func Launch(file, template string, system func(string) error) error {
	if strings.TrimSpace(template) == "" {
		return system(file)
	}
	program, args, err := Command(template, file)
	if err != nil {
		return err
	}
	cmd := exec.Command(program, args...)
	if err = cmd.Start(); err != nil {
		return fmt.Errorf("启动编辑器失败: %w", err)
	}
	go func() { _ = cmd.Wait() }()
	return nil
}
func Editors(settings model.Settings) []model.Editor {
	out := []model.Editor{{Name: "系统默认程序", Command: "__system__"}}
	add := func(name, command string) {
		if strings.TrimSpace(command) == "" {
			return
		}
		for _, e := range out {
			if e.Command == command {
				return
			}
		}
		out = append(out, model.Editor{Name: name, Command: command})
	}
	candidates := []model.Editor{{Name: "VS Code", Command: "code {file}"}, {Name: "Cursor", Command: "cursor {file}"}, {Name: "Zed", Command: "zed {file}"}, {Name: "Sublime Text", Command: "subl {file}"}, {Name: "Helix", Command: "hx {file}"}, {Name: "Neovim", Command: "nvim {file}"}, {Name: "Vim", Command: "vim {file}"}, {Name: "Emacs", Command: "emacs {file}"}, {Name: "nano", Command: "nano {file}"}, {Name: "Micro", Command: "micro {file}"}}
	if runtime.GOOS == "windows" {
		candidates = append(candidates, model.Editor{Name: "Notepad++", Command: "notepad++ {file}"}, model.Editor{Name: "Notepad", Command: "notepad {file}"})
	}
	if runtime.GOOS == "linux" {
		candidates = append(candidates, model.Editor{Name: "gedit", Command: "gedit {file}"}, model.Editor{Name: "Kate", Command: "kate {file}"})
	}
	for _, e := range candidates {
		program, _, err := Command(e.Command, "file")
		if err == nil {
			if _, err = exec.LookPath(program); err == nil {
				add(e.Name, e.Command)
			}
		}
	}
	if runtime.GOOS == "darwin" {
		for _, entry := range []struct{ name, bundle string }{{"VS Code", "/Applications/Visual Studio Code.app"}, {"Cursor", "/Applications/Cursor.app"}, {"Zed", "/Applications/Zed.app"}, {"Sublime Text", "/Applications/Sublime Text.app"}, {"TextEdit", "/System/Applications/TextEdit.app"}, {"BBEdit", "/Applications/BBEdit.app"}, {"TextMate", "/Applications/TextMate.app"}, {"Xcode", "/Applications/Xcode.app"}} {
			if info, err := os.Stat(entry.bundle); err == nil && info.IsDir() {
				name := strings.TrimSuffix(filepath.Base(entry.bundle), ".app")
				command := "/usr/bin/open -a \"" + name + "\" {file}"
				found := false
				for _, e := range out {
					found = found || e.Name == entry.name
				}
				if !found {
					add(entry.name, command)
				}
			}
		}
	}
	for _, key := range []string{"VISUAL", "EDITOR"} {
		if command := strings.TrimSpace(os.Getenv(key)); command != "" {
			add("环境变量编辑器", command)
			break
		}
	}
	for _, e := range settings.Editors {
		add(e.Name, e.Command)
	}
	add("默认编辑器", settings.DefaultEditor)
	return out
}
func Resolve(settings model.Settings, choice string) (string, error) {
	if choice == "" {
		choice = settings.DefaultEditor
	}
	if choice == "" || choice == "__system__" {
		return "", nil
	}
	for _, e := range Editors(settings) {
		if e.Command == choice {
			if _, _, err := Command(choice, "file"); err != nil {
				return "", err
			}
			return choice, nil
		}
	}
	return "", fmt.Errorf("编辑器不在当前配置或本机探测列表中")
}
