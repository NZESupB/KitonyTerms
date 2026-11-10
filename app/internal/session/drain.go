package session

import (
	"context"
	"fmt"
	"sync"
)

// 所有工作准入都在 m.mu 内完成，等待清理时不会再发生迟到的 Add。
func (m *Manager) addWorkerLocked() {
	if m.workers == 0 {
		m.idle = make(chan struct{})
	}
	m.workers++
	m.wg.Add(1)
}

func (m *Manager) workerDone() {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.workers--
	m.wg.Done()
	if m.workers == 0 {
		close(m.idle)
	}
}

// Drain 取消现有连接并等待实际工作完成；返回的释放函数只重新开放自己的准入门。
// 失败会恢复准入，已断开的会话可由用户重新连接，Manager 的生命周期不会结束。
func (m *Manager) Drain(ctx context.Context) (func(), error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	m.mu.Lock()
	if m.closed || m.draining != "" || m.ctx.Err() != nil {
		m.mu.Unlock()
		return nil, fmt.Errorf("连接管理器已关闭或正在收尾")
	}
	owner := randomID()
	m.draining = owner
	all := make([]*Session, 0, len(m.sessions))
	for _, s := range m.sessions {
		all = append(all, s)
	}
	idle := m.idle
	workers := m.workers
	m.mu.Unlock()
	var once sync.Once
	release := func() {
		once.Do(func() {
			m.mu.Lock()
			if m.draining == owner {
				m.draining = ""
			}
			m.mu.Unlock()
		})
	}
	for _, s := range all {
		s.mu.Lock()
		if s.status != "closed" {
			s.status = "closing"
		}
		s.mu.Unlock()
		s.cancel()
	}
	if m.notify != nil {
		m.notify()
	}
	if workers != 0 {
		select {
		case <-idle:
		case <-ctx.Done():
			release()
			return nil, fmt.Errorf("连接清理未完成：%w", ctx.Err())
		}
	}
	if err := ctx.Err(); err != nil {
		release()
		return nil, err
	}
	return release, nil
}
