package ipc

import (
	"context"
	"fmt"

	"github.com/wailsapp/wails/v3/pkg/application"
	"kitonyterms/internal/externaledit"
	"kitonyterms/internal/model"
)

func (b *Backend) GetEditors() ([]model.Editor, error) {
	if err := b.check(); err != nil {
		return nil, err
	}
	if application.System.IsMobile() {
		return []model.Editor{}, nil
	}
	return externaledit.Editors(b.repo.Snapshot().Config.Settings), nil
}
func (b *Backend) OpenExternalEdit(id, generation, target, editor string) (model.ExternalEdit, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.ExternalEdit{}, err
	}
	defer done()
	if application.System.IsMobile() {
		return model.ExternalEdit{}, fmt.Errorf("手机请使用内置编辑器")
	}
	if _, err := b.manager.Get(id, generation); err != nil {
		return model.ExternalEdit{}, err
	}
	command, err := externaledit.Resolve(b.repo.Snapshot().Config.Settings, editor)
	if err != nil {
		return model.ExternalEdit{}, err
	}
	return b.edits.Start(id, generation, target, command)
}
func (b *Backend) GetExternalEdits() ([]model.ExternalEdit, error) {
	if err := b.check(); err != nil {
		return nil, err
	}
	return b.edits.List(), nil
}
func (b *Backend) ExternalEditAction(ctx context.Context, id, generation, editID, action string) error {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return err
	}
	defer done()
	return b.edits.Action(ctx, id, generation, editID, action)
}
