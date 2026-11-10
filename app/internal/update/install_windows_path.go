package update

import (
	"fmt"
	"strings"
	"unicode/utf16"
	"unicode/utf8"
)

// 仅用于 Shell/Inno 边界；事务记录与摘要继续使用原规范路径。
func windowsExternalPath(path string) (string, error) {
	bad := func() (string, error) {
		return "", fmt.Errorf("路径无法安全传给系统安装器，请使用较短的普通路径")
	}
	if !utf8.ValidString(path) || strings.ContainsAny(path, "/\x00") {
		return bad()
	}
	simple := path
	if strings.HasPrefix(simple, `\\?\UNC\`) {
		simple = `\\` + strings.TrimPrefix(simple, `\\?\UNC\`)
	} else if strings.HasPrefix(simple, `\\?\`) {
		simple = strings.TrimPrefix(simple, `\\?\`)
	}
	var parts []string
	if len(simple) >= 3 && ((simple[0] >= 'A' && simple[0] <= 'Z') || (simple[0] >= 'a' && simple[0] <= 'z')) && simple[1:3] == `:\` {
		if len(simple) > 3 {
			parts = strings.Split(simple[3:], `\`)
		}
	} else if strings.HasPrefix(simple, `\\`) {
		parts = strings.Split(simple[2:], `\`)
		if len(parts) < 2 {
			return bad()
		}
	} else {
		return bad()
	}
	for _, part := range parts {
		if !archiveComponent(part) {
			return bad()
		}
	}
	if len(utf16.Encode([]rune(simple))) >= 260 {
		return bad()
	}
	return simple, nil
}

func windowsCommandArguments(arguments []string) (string, error) {
	quoted := make([]string, 0, len(arguments))
	for _, value := range arguments {
		if !utf8.ValidString(value) || strings.ContainsRune(value, 0) {
			return "", fmt.Errorf("系统参数包含无效字符")
		}
		var text strings.Builder
		text.WriteByte('"')
		slashes := 0
		for _, c := range value {
			if c == '\\' {
				slashes++
				continue
			}
			count := slashes
			if c == '"' {
				count = slashes*2 + 1
			}
			text.WriteString(strings.Repeat(`\`, count))
			text.WriteRune(c)
			slashes = 0
		}
		text.WriteString(strings.Repeat(`\`, slashes*2))
		text.WriteByte('"')
		quoted = append(quoted, text.String())
	}
	result := strings.Join(quoted, " ")
	if len(utf16.Encode([]rune(result))) >= 32767 {
		return "", fmt.Errorf("系统参数过长")
	}
	return result, nil
}

func windowsInstallerArguments(directory, log string) ([]string, error) {
	directory, err := windowsExternalPath(directory)
	if err != nil {
		return nil, err
	}
	log, err = windowsExternalPath(log)
	if err != nil {
		return nil, err
	}
	return []string{"/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", "/NOCLOSEAPPLICATIONS", "/NORESTARTAPPLICATIONS", "/DIR=" + directory, "/LOG=" + log}, nil
}
