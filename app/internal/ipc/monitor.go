package ipc

import "kitonyterms/internal/model"

func (b *Backend) PollMonitor(id, generation, watchID string) (model.MonitorSnapshot, error) {
	if err := b.check(); err != nil {
		return model.MonitorSnapshot{}, err
	}
	return b.manager.PollMonitor(id, generation, watchID)
}
func (b *Backend) StopMonitor(id, generation, watchID string) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.StopMonitor(id, generation, watchID)
}
