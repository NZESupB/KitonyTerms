package ipc

import (
	"context"

	"github.com/wailsapp/wails/v3/pkg/application"
	"kitonyterms/internal/model"
)

func (b *Backend) CreateDirectory(ctx context.Context, id, generation, target string) error {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return err
	}
	defer done()
	return b.manager.CreateDirectory(ctx, id, generation, target)
}
func (b *Backend) RenamePath(ctx context.Context, id, generation, source, destination string) error {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return err
	}
	defer done()
	return b.manager.RenamePath(ctx, id, generation, source, destination)
}
func (b *Backend) RemovePaths(ctx context.Context, id, generation string, targets []string) error {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return err
	}
	defer done()
	return b.manager.RemovePaths(ctx, id, generation, targets)
}
func (b *Backend) DownloadBatch(ctx context.Context, id, generation string, remotes []string) (model.TransferSummary, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.TransferSummary{}, err
	}
	defer done()
	if application.System.IsMobile() {
		return b.mobileDownload(ctx, id, generation, remotes, "")
	}
	destination, err := b.app.Dialog.OpenFile().AttachToWindow(b.window).CanChooseFiles(false).CanChooseDirectories(true).PromptForSingleSelection()
	if err != nil || destination == "" {
		return model.TransferSummary{}, err
	}
	return b.manager.DownloadBatch(ctx, id, generation, remotes, destination)
}

func (b *Backend) StartUpload(ctx context.Context, id, generation, dir string, directory, overwrite bool) (model.TransferProgress, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.TransferProgress{}, err
	}
	defer done()
	if application.System.IsMobile() {
		return b.startMobileUpload(ctx, id, generation, dir, directory, overwrite)
	}
	dialog := b.app.Dialog.OpenFile().AttachToWindow(b.window).CanChooseFiles(!directory).CanChooseDirectories(directory)
	locals, err := dialog.PromptForMultipleSelection()
	if err != nil || len(locals) == 0 {
		return model.TransferProgress{}, err
	}
	if err = ctx.Err(); err != nil {
		return model.TransferProgress{}, err
	}
	return b.manager.StartUpload(id, generation, dir, locals, overwrite)
}
func (b *Backend) StartDownload(ctx context.Context, id, generation string, remotes []string) (model.TransferProgress, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.TransferProgress{}, err
	}
	defer done()
	if application.System.IsMobile() {
		return b.startMobileDownload(ctx, id, generation, remotes)
	}
	destination, err := b.app.Dialog.OpenFile().AttachToWindow(b.window).CanChooseFiles(false).CanChooseDirectories(true).PromptForSingleSelection()
	if err != nil || destination == "" {
		return model.TransferProgress{}, err
	}
	if err = ctx.Err(); err != nil {
		return model.TransferProgress{}, err
	}
	return b.manager.StartDownload(id, generation, remotes, destination)
}
func (b *Backend) GetTransfers(id, generation string) ([]model.TransferProgress, error) {
	if err := b.check(); err != nil {
		return nil, err
	}
	return b.manager.Transfers(id, generation)
}
func (b *Backend) CancelTransfer(id, generation, transferID string) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.CancelTransfer(id, generation, transferID)
}
