package filepolicy

import (
	"strings"
	"unicode"
	"unicode/utf8"
)

// Name 只接受可跨平台保存的单段文件名，不改变用户选中的名称。
func Name(name string) bool {
	if !utf8.ValidString(name) || len(name) > 255 || name == "" || name == "." || name == ".." || strings.ContainsAny(name, "<>:\"/\\|?*") || strings.TrimRight(name, ". ") != name {
		return false
	}
	for _, r := range name {
		if unicode.IsControl(r) {
			return false
		}
	}
	stem := strings.ToUpper(strings.SplitN(name, ".", 2)[0])
	switch stem {
	case "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$":
		return false
	}
	runes := []rune(stem)
	return !(len(runes) == 4 && (strings.HasPrefix(stem, "COM") || strings.HasPrefix(stem, "LPT")) && strings.ContainsRune("123456789¹²³", runes[3]))
}
