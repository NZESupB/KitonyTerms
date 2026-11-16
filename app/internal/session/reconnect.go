package session

import (
	"context"
	"fmt"

	"kitonyterms/internal/model"
	"kitonyterms/internal/shellintegration"
)

func (m *Manager) newSession(id string, profile model.Profile) *Session {
	ctx, cancel := context.WithCancel(m.ctx)
	return &Session{id: id, generation: randomID(), profile: profile, status: "connecting", ctx: ctx, cancel: cancel, done: make(chan struct{}), output: newOutput(), shell: shellintegration.NewObserver()}
}

// Reconnect 保留标签页身份，用独立代次隔离旧连接的请求和运行状态。
func (m *Manager) Reconnect(id, generation string) (model.SessionSnapshot, error) {
	previous, err := m.Get(id, generation)
	if err != nil {
		return model.SessionSnapshot{}, err
	}
	snapshot := previous.snapshot()
	if snapshot.Status != "closed" {
		return model.SessionSnapshot{}, fmt.Errorf("只能重新连接已断开的会话")
	}
	profile, err := m.repo.Profile(snapshot.Title)
	if err != nil {
		return model.SessionSnapshot{}, err
	}
	next := m.newSession(id, profile)
	m.mu.Lock()
	if m.closed || m.draining != "" || m.ctx.Err() != nil {
		m.mu.Unlock()
		next.cancel()
		return model.SessionSnapshot{}, fmt.Errorf("应用正在关闭")
	}
	if m.sessions[id] != previous {
		m.mu.Unlock()
		next.cancel()
		return model.SessionSnapshot{}, fmt.Errorf("会话或连接代次已失效")
	}
	previous.mu.Lock()
	closed := previous.status == "closed"
	previous.mu.Unlock()
	if !closed {
		m.mu.Unlock()
		next.cancel()
		return model.SessionSnapshot{}, fmt.Errorf("只能重新连接已断开的会话")
	}
	m.sessions[id] = next
	m.addWorkerLocked()
	m.mu.Unlock()
	previous.cancel()
	m.startSession(next)
	return next.snapshot(), nil
}
