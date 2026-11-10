package ipc

import (
	"context"
	"kitonyterms/internal/model"
)

func (b *Backend) StartContainerTerminal(id, generation string, request model.ContainerTerminalRequest) (model.ContainerTerminalSnapshot, error) {
	if err := b.check(); err != nil {
		return model.ContainerTerminalSnapshot{}, err
	}
	return b.manager.StartContainerTerminal(id, generation, request)
}
func (b *Backend) GetContainerTerminal(id, generation, execID string) (model.ContainerTerminalSnapshot, error) {
	if err := b.check(); err != nil {
		return model.ContainerTerminalSnapshot{}, err
	}
	return b.manager.GetContainerTerminal(id, generation, execID)
}
func (b *Backend) PollContainerTerminal(id, generation, execID, after string) (model.TerminalBatch, error) {
	if err := b.check(); err != nil {
		return model.TerminalBatch{}, err
	}
	return b.manager.PollContainerTerminal(id, generation, execID, after)
}
func (b *Backend) SendContainerInput(ctx context.Context, request model.ContainerInputRequest) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.SendContainerInput(ctx, request)
}
func (b *Backend) ResizeContainerTerminal(ctx context.Context, id, generation, execID string, cols, rows int) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.ResizeContainerTerminal(ctx, id, generation, execID, cols, rows)
}
func (b *Backend) CloseContainerTerminal(id, generation, execID string) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.CloseContainerTerminal(id, generation, execID)
}
