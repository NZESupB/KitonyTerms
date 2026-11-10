package shellintegration

import (
	"bytes"
	"strings"
	"time"
)

const PromptPayload = "1337;KitonyTermsPrompt=ready"
const DonePayload = "1337;KitonyTermsShellIntegration=done"
const PromptMarker = "\x1b]" + PromptPayload + "\a"
const DoneMarker = "\x1b]" + DonePayload + "\a"

// 与旧客户端使用同一提示符协议；追加 hook，不覆盖用户的标量或数组配置。
const Bootstrap = `__kt_cwd(){ printf '\033]7;file://%s\a' "$PWD"; if [ "${1:-}" != bootstrap ] && [ -n "${BASH_VERSION:-}${ZSH_VERSION:-}" ]; then printf '\033]1337;KitonyTermsPrompt=ready\a'; fi; }; ` +
	`if [ -n "$ZSH_VERSION" ]; then eval 'precmd_functions+=(__kt_cwd); setopt hist_ignore_space'; ` +
	`elif [ -n "$BASH_VERSION" ]; then case "$(declare -p PROMPT_COMMAND 2>/dev/null)" in ` +
	`"declare -a"*) eval 'PROMPT_COMMAND+=(__kt_cwd)';; *) PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND;}__kt_cwd";; esac; ` +
	`HISTCONTROL="${HISTCONTROL:+$HISTCONTROL:}ignorespace"; fi; __kt_cwd bootstrap; printf '\033]1337;KitonyTermsShellIntegration=done\a'` + "\n"

func DirectoryCommand(path string) string {
	path = strings.TrimSpace(path)
	if path == "" || strings.ContainsAny(path, "\x00\r\n") {
		return ""
	}
	prefix := "\x15 printf '\\033[A\\r\\033[J'; cd"
	if path == "~" || path == "." {
		return prefix + "\n"
	}
	return prefix + " -- '" + strings.ReplaceAll(path, "'", "'\\''") + "'\n"
}

type BootstrapFilter struct {
	pending  []byte
	deadline time.Time
	echo     bool
}

func (f *BootstrapFilter) Start(now time.Time) {
	f.pending = nil
	f.deadline = now.Add(3 * time.Second)
	f.echo = false
}
func (f *BootstrapFilter) Active() bool { return !f.deadline.IsZero() }
func (f *BootstrapFilter) Finish() []byte {
	pending := f.pending
	f.pending = nil
	f.deadline = time.Time{}
	f.echo = false
	return pending
}
func (f *BootstrapFilter) tail() []byte {
	if pos := bytes.Index(f.pending, []byte(DoneMarker)); pos >= 0 {
		result := bytes.Clone(f.pending[pos+len(DoneMarker):])
		f.Finish()
		return result
	}
	if len(f.pending) > 64<<10 {
		return f.Finish()
	}
	return nil
}
func (f *BootstrapFilter) Filter(data []byte, now time.Time) []byte {
	if !f.Active() {
		return data
	}
	if !now.Before(f.deadline) {
		return append(f.Finish(), data...)
	}
	f.pending = append(f.pending, data...)
	if f.echo {
		return f.tail()
	}
	echo := []byte(strings.TrimSuffix(Bootstrap, "\n"))
	if start, end := findEcho(f.pending, echo); start >= 0 {
		visible := append(bytes.Clone(f.pending[:start]), []byte("\r\x1b[2K")...)
		f.pending = bytes.Clone(f.pending[end:])
		f.echo = true
		return append(visible, f.tail()...)
	}
	if pos := bytes.Index(f.pending, []byte(DoneMarker)); pos >= 0 {
		visible := append(bytes.Clone(f.pending[:pos]), []byte("\r\x1b[2K")...)
		visible = append(visible, f.pending[pos+len(DoneMarker):]...)
		f.Finish()
		return visible
	}
	if len(f.pending) > 64<<10 {
		return f.Finish()
	}
	keep := max(suffixPrefix(f.pending, echo, true), suffixPrefix(f.pending, []byte(DoneMarker), false))
	visible := bytes.Clone(f.pending[:len(f.pending)-keep])
	f.pending = bytes.Clone(f.pending[len(f.pending)-keep:])
	return visible
}
func findEcho(data, needle []byte) (int, int) {
	for start := range data {
		if data[start] != needle[0] {
			continue
		}
		cursor, matched := start, 0
		for cursor < len(data) && matched < len(needle) {
			b := data[cursor]
			if b == '\r' || b == '\n' {
				cursor++
				continue
			}
			if b != needle[matched] {
				break
			}
			cursor++
			matched++
		}
		if matched == len(needle) {
			return start, cursor
		}
	}
	return -1, -1
}
func suffixPrefix(data, needle []byte, ignoreLines bool) int {
	if len(needle) == 0 {
		return 0
	}
	border := make([]int, len(needle))
	for i, matched := 1, 0; i < len(needle); i++ {
		for matched > 0 && needle[i] != needle[matched] {
			matched = border[matched-1]
		}
		if needle[i] == needle[matched] {
			matched++
		}
		border[i] = matched
	}
	positions := make([]int, len(needle))
	matched, seen := 0, 0
	for i, b := range data {
		if ignoreLines && (b == '\r' || b == '\n') {
			continue
		}
		for matched > 0 && (matched == len(needle) || needle[matched] != b) {
			matched = border[matched-1]
		}
		if needle[matched] == b {
			matched++
		}
		positions[seen%len(needle)] = i
		seen++
	}
	if matched == 0 || matched == len(needle) {
		return 0
	}
	return len(data) - positions[(seen-matched)%len(needle)]
}
