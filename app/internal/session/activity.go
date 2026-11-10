package session

import "kitonyterms/internal/model"

func (m *Manager) Activity() model.AppActivity {
	m.mu.Lock()
	sessions := make([]*Session, 0, len(m.sessions))
	for _, s := range m.sessions {
		sessions = append(sessions, s)
	}
	m.mu.Unlock()
	var result model.AppActivity
	for _, s := range sessions {
		s.mu.Lock()
		if s.status != "closed" {
			result.Sessions++
		}
		result.Operations += len(s.operations)
		transfers := append([]*transferJob(nil), s.transfers...)
		child := s.container
		s.mu.Unlock()
		for _, job := range transfers {
			if !terminalTransfer(job.snapshot().Status) {
				result.Transfers++
			}
		}
		if child != nil {
			child.mu.Lock()
			if child.status != "closed" {
				result.ContainerTerminals++
			}
			child.mu.Unlock()
		}
	}
	return result
}
