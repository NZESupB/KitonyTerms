package session

import (
	"errors"
	"fmt"
	"io"
	"strings"
	"time"

	"kitonyterms/internal/model"
	"kitonyterms/internal/shellintegration"
)

func (m *Manager) ShellState(id, generation string) (model.ShellState, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return model.ShellState{}, err
	}
	return s.shell.Snapshot(time.Now()), nil
}
func (m *Manager) CommandHistory(id, generation string) ([]model.CommandEntry, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return nil, err
	}
	return s.shell.History(), nil
}
func (m *Manager) ClearCommandHistory(id, generation string) error {
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	s.shell.ClearHistory()
	return nil
}

// 文件浏览器的手动与自动同步共用此入口，内部写入不推进用户输入序号。
func (m *Manager) ChangeDirectory(id, generation, target string, automatic bool) (bool, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return false, err
	}
	target = strings.TrimSpace(target)
	command := shellintegration.DirectoryCommand(target)
	if command == "" || len(target) > 4096 || (!strings.HasPrefix(target, "/") && target != "~" && target != ".") {
		return false, fmt.Errorf("同步目录必须为有效的绝对路径或home缩写")
	}
	s.inputMu.Lock()
	defer s.inputMu.Unlock()
	s.mu.Lock()
	stdin, runtime, status := s.stdin, s.shellIO, s.status
	s.mu.Unlock()
	if stdin == nil || runtime == nil || status != "connected" || s.ctx.Err() != nil {
		return false, fmt.Errorf("终端尚未连接")
	}
	if _, err = runtime.observe(nil, target); err != nil {
		if automatic && errors.Is(err, shellintegration.ErrAltScreen) {
			return false, nil
		}
		return false, err
	}
	n, err := stdin.Write([]byte(command))
	if err != nil || n != len(command) {
		s.cancel()
		if err != nil {
			return false, err
		}
		return false, io.ErrShortWrite
	}
	return true, nil
}
