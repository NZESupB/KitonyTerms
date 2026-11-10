package session

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"fmt"
	"io"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
	"kitonyterms/internal/shellintegration"
)

func readFixtureLine(reader *bufio.Reader) (string, error) {
	var line []byte
	for {
		b, err := reader.ReadByte()
		if err != nil {
			return "", err
		}
		if b == '\r' || b == '\n' {
			return string(line), nil
		}
		if b == 0x15 {
			line = line[:0]
		} else {
			line = append(line, b)
		}
		if len(line) > 64<<10 {
			return "", fmt.Errorf("夹具输入超限")
		}
	}
}
func waitShell(t *testing.T, m *Manager, s model.SessionSnapshot, match func(model.ShellState) bool) model.ShellState {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		state, err := m.ShellState(s.ID, s.Generation)
		if err != nil {
			t.Fatal(err)
		}
		if match(state) {
			return state
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("shell状态未收敛")
	return model.ShellState{}
}
func waitShellOutput(t *testing.T, m *Manager, s model.SessionSnapshot, after *string, wanted string) string {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	var output strings.Builder
	for time.Now().Before(deadline) {
		batch, err := m.Poll(s.ID, s.Generation, *after)
		if err != nil {
			t.Fatal(err)
		}
		for _, chunk := range batch.Chunks {
			data, err := base64.StdEncoding.DecodeString(chunk.Data)
			if err != nil {
				t.Fatal(err)
			}
			output.Write(data)
			*after = chunk.Sequence
		}
		if strings.Contains(output.String(), wanted) {
			return output.String()
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("未收到shell输出")
	return ""
}

func TestShellHistoryDirectoryAndInputReplayRoundtrip(t *testing.T) {
	var directoryWrites atomic.Int32
	server := startLoopbackHooks(t, nil, nil, func(ch ssh.Channel, req *ssh.Request) bool {
		if req.Type != "shell" {
			return false
		}
		_ = req.Reply(true, nil)
		go func() {
			defer ch.Close()
			reader := bufio.NewReader(ch)
			_, _ = io.WriteString(ch, "Welcome\r\nLast login\r\n$ ")
			bootstrap, err := reader.ReadBytes('\n')
			if err != nil {
				return
			}
			if !bytes.Equal(bootstrap, []byte(shellintegration.Bootstrap)) {
				t.Error("未收到约定bootstrap")
				return
			}
			_, _ = ch.Write(bootstrap)
			_, _ = io.WriteString(ch.Stderr(), "shell-diagnostic\r\n")
			_, _ = io.WriteString(ch, "\x1b]7;file:///home/test\a"+shellintegration.DoneMarker+shellintegration.PromptMarker+"$ ")
			awaitingSecret := false
			for {
				line, err := readFixtureLine(reader)
				if err != nil {
					return
				}
				if awaitingSecret {
					awaitingSecret = false
					_, _ = io.WriteString(ch, "root\r\n"+shellintegration.PromptMarker+"$ ")
					continue
				}
				switch {
				case line == "sudo whoami":
					awaitingSecret = true
					_, _ = io.WriteString(ch, "Password: ")
				case line == "enter-alt":
					_, _ = io.WriteString(ch, "\x1b[?1049h")
				case line == "leave-alt":
					_, _ = io.WriteString(ch, "\x1b[?1049l"+shellintegration.PromptMarker+"$ ")
				case strings.HasPrefix(line, " printf"):
					directoryWrites.Add(1)
					_, _ = io.WriteString(ch, "\x1b]7;file:///home/test\a\x1b]7;file:///srv/%E4%B8%AD\a"+shellintegration.PromptMarker+"$ ")
				default:
					_, _ = io.WriteString(ch, line+"\r\n"+shellintegration.PromptMarker+"$ ")
				}
			}
		}()
		return true
	})
	m, _ := newLoopbackManager(t, server, "tester")
	opened, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	s := waitConnected(t, m, opened, true)
	after := "0"
	initial := waitShellOutput(t, m, s, &after, shellintegration.PromptMarker)
	if !strings.Contains(initial, "Welcome") || strings.Contains(initial, "__kt_cwd") {
		t.Fatalf("bootstrap过滤错误: %q", initial)
	}
	state := waitShell(t, m, s, func(value model.ShellState) bool { return value.Cwd == "/home/test" })
	if state.Revision != "1" {
		t.Fatal(state)
	}
	sequence := uint64(0)
	send := func(text string) model.InputRequest {
		t.Helper()
		sequence++
		request := model.InputRequest{SessionID: s.ID, Generation: s.Generation, Sequence: strconv.FormatUint(sequence, 10), Data: base64.StdEncoding.EncodeToString([]byte(text))}
		if err := m.Input(request); err != nil {
			t.Fatal(err)
		}
		return request
	}
	first := send("pwd\r")
	waitShellOutput(t, m, s, &after, shellintegration.PromptMarker)
	if err = m.Input(first); err != nil {
		t.Fatal("重试未去重", err)
	}
	send("echo 中文\r")
	waitShellOutput(t, m, s, &after, shellintegration.PromptMarker)
	send("sudo whoami\r")
	waitShellOutput(t, m, s, &after, "Password:")
	send("synthetic-private-value\r")
	waitShellOutput(t, m, s, &after, shellintegration.PromptMarker)
	send("enter-alt\r")
	waitShell(t, m, s, func(value model.ShellState) bool { return value.AltScreen })
	if written, err := m.ChangeDirectory(s.ID, s.Generation, "/srv/中", true); err != nil || written {
		t.Fatal("备用屏自动同步写入了PTY", written, err)
	}
	if _, err = m.ChangeDirectory(s.ID, s.Generation, "/srv/中", false); err == nil {
		t.Fatal("手动同步未解释备用屏拒绝")
	}
	send("full-screen-private\r")
	send("leave-alt\r")
	waitShell(t, m, s, func(value model.ShellState) bool { return !value.AltScreen })
	waitShellOutput(t, m, s, &after, shellintegration.PromptMarker)
	send(" hidden\r")
	waitShellOutput(t, m, s, &after, shellintegration.PromptMarker)
	if written, err := m.ChangeDirectory(s.ID, s.Generation, "/srv/中", false); err != nil || !written {
		t.Fatal(written, err)
	}
	waitShell(t, m, s, func(value model.ShellState) bool { return value.Cwd == "/srv/中" && value.PendingDirectory == "" })
	waitShellOutput(t, m, s, &after, shellintegration.PromptMarker)
	input, err := m.TerminalState(s.ID, s.Generation)
	if err != nil || input.InputSequence != strconv.FormatUint(sequence, 10) {
		t.Fatal("内部cd占用了用户序号", input, err)
	}
	history, err := m.CommandHistory(s.ID, s.Generation)
	if err != nil {
		t.Fatal(err)
	}
	if len(history) != 4 || history[0].Command != "pwd" || history[1].Command != "echo 中文" || history[2].Command != "sudo whoami" || history[3].Command != "enter-alt" {
		t.Fatalf("历史边界错误: %+v", history)
	}
	if directoryWrites.Load() != 1 {
		t.Fatal("目录写入次数错误", directoryWrites.Load())
	}
	if _, err = m.CommandHistory(s.ID, "stale"); err == nil {
		t.Fatal("旧代次读取历史")
	}
	if _, err = m.ChangeDirectory(s.ID, "stale", "/srv/中", false); err == nil {
		t.Fatal("旧代次切目录")
	}
	if err = m.ClearCommandHistory(s.ID, s.Generation); err != nil {
		t.Fatal(err)
	}
	if history, err = m.CommandHistory(s.ID, s.Generation); err != nil || len(history) != 0 {
		t.Fatal("清空失败", err)
	}
	send("pwd\r")
	waitShellOutput(t, m, s, &after, shellintegration.PromptMarker)
	history, _ = m.CommandHistory(s.ID, s.Generation)
	if len(history) != 1 || history[0].Sequence != "5" {
		t.Fatal("清空后序号复用", history)
	}
	m.Shutdown()
	if m.Activity() != (model.AppActivity{}) {
		t.Fatal("shutdown未回收shell任务")
	}
}
