package ipc

import "kitonyterms/internal/model"

func (b *Backend) GetShellState(id, generation string) (model.ShellState, error) {
	if err := b.check(); err != nil {
		return model.ShellState{}, err
	}
	return b.manager.ShellState(id, generation)
}
func (b *Backend) GetCommandHistory(id, generation string) ([]model.CommandEntry, error) {
	if err := b.check(); err != nil {
		return nil, err
	}
	return b.manager.CommandHistory(id, generation)
}
func (b *Backend) ClearCommandHistory(id, generation string) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.ClearCommandHistory(id, generation)
}
func (b *Backend) ChangeDirectory(id, generation, target string, automatic bool) (bool, error) {
	if err := b.check(); err != nil {
		return false, err
	}
	return b.manager.ChangeDirectory(id, generation, target, automatic)
}
