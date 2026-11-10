package shellintegration

import (
	"errors"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"kitonyterms/internal/model"
)

var ErrAltScreen = errors.New("终端正在运行全屏程序，已跳过目录同步")

type Observer struct {
	mu           sync.Mutex
	protocol     controlParser
	terminal     controlParser
	initialized  bool
	ready        bool
	invalid      bool
	line         []byte
	cwd          string
	revision     uint64
	pending      string
	pendingUntil time.Time
	sequence     uint64
	history      []model.CommandEntry
}

func NewObserver() *Observer {
	return &Observer{protocol: controlParser{capture: true}}
}

func (o *Observer) wipeLine() {
	clear(o.line)
	o.line = o.line[:0]
}

func (o *Observer) outputOSC(payload string, now time.Time) {
	switch payload {
	case DonePayload:
		o.initialized = true
	case PromptPayload:
		if o.initialized && !o.terminal.alt {
			o.wipeLine()
			o.invalid = false
			o.ready = true
		}
	default:
		cwd := decodeCwd(payload)
		if cwd == "" {
			return
		}
		if o.pending != "" && now.Before(o.pendingUntil) && cwd != o.pending {
			return
		}
		o.pending = ""
		if cwd != o.cwd {
			o.cwd = cwd
			o.revision++
		}
	}
}

// 输出和输入观察由会话队列排序；快照/历史查询使用短临界区，不依赖终端背压。
func (o *Observer) Output(data []byte, stdout bool, now time.Time) {
	o.mu.Lock()
	defer o.mu.Unlock()
	accept := func(payload string) { o.outputOSC(payload, now) }
	for i := range data {
		value := data[i : i+1]
		o.terminal.feed(value, nil)
		if o.terminal.alt {
			o.ready = false
			o.wipeLine()
		}
		if stdout {
			o.protocol.feed(value, accept)
		}
	}
}

func (o *Observer) Input(data []byte) string {
	o.mu.Lock()
	defer o.mu.Unlock()
	if len(data) > 0 {
		o.pending = ""
	}
	if o.terminal.alt {
		o.ready = false
		o.wipeLine()
	}
	if !o.ready {
		return ""
	}
	for _, b := range data {
		switch {
		case b == '\r' || b == '\n':
			o.ready = false
			var command string
			if !o.invalid && len(o.line) > 0 && o.line[0] != ' ' && utf8.Valid(o.line) && strings.TrimSpace(string(o.line)) != "" {
				command = string(o.line)
			}
			o.wipeLine()
			return command
		case (b == 0x7f || b == 0x08) && !o.invalid:
			if len(o.line) > 0 {
				o.line = o.line[:len(o.line)-1]
			}
			for len(o.line) > 0 && !utf8.Valid(o.line) {
				o.line = o.line[:len(o.line)-1]
			}
		case b == 0x15:
			o.wipeLine()
		case b >= 0x20 && b != 0x7f && !o.invalid && len(o.line) < 4096:
			o.line = append(o.line, b)
		default:
			o.wipeLine()
			o.invalid = true
		}
	}
	return ""
}

// 只有PTY写入成功后提交，合并粘贴的后续行及程序密码输入不会成为历史。
func (o *Observer) Commit(command string, now time.Time) {
	if command == "" {
		return
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	o.sequence++
	o.history = append(o.history, model.CommandEntry{Sequence: strconv.FormatUint(o.sequence, 10), Time: now.Format("15:04:05"), Command: command})
	if len(o.history) > 200 {
		copy(o.history, o.history[len(o.history)-200:])
		o.history = o.history[:200]
	}
}

func (o *Observer) Directory(path string, now time.Time) error {
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.terminal.alt {
		return ErrAltScreen
	}
	o.ready = false
	o.wipeLine()
	o.pending = path
	if path == "~" || path == "." {
		o.pending = ""
	}
	o.pendingUntil = now.Add(3 * time.Second)
	return nil
}

func (o *Observer) Snapshot(now time.Time) model.ShellState {
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.pending != "" && !now.Before(o.pendingUntil) {
		o.pending = ""
	}
	return model.ShellState{Cwd: o.cwd, Revision: strconv.FormatUint(o.revision, 10), AltScreen: o.terminal.alt, PendingDirectory: o.pending}
}

func (o *Observer) History() []model.CommandEntry {
	o.mu.Lock()
	defer o.mu.Unlock()
	return append([]model.CommandEntry{}, o.history...)
}

func (o *Observer) ClearHistory() {
	o.mu.Lock()
	defer o.mu.Unlock()
	clear(o.history)
	o.history = nil
}
