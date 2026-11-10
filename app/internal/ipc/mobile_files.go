package ipc

import (
	"context"
	"errors"
	"fmt"

	"kitonyterms/internal/mobilefiles"
	"kitonyterms/internal/model"
	"kitonyterms/internal/session"
)

func (b *Backend) newMobileStage(id, generation string) (*mobilefiles.Stage, error) {
	if b.mobileFiles == nil {
		return nil, mobilefiles.ErrUnavailable
	}
	if _, err := b.manager.Get(id, generation); err != nil {
		return nil, err
	}
	return b.mobileFiles.NewStage()
}
func closeMobileStage(ctx context.Context, stage *mobilefiles.Stage, operationErr error) error {
	cleanupErr := stage.Close()
	if cleanupErr == nil && ctx.Err() == nil && errors.Is(operationErr, mobilefiles.ErrCancelled) {
		return nil
	}
	return errors.Join(operationErr, cleanupErr)
}
func (b *Backend) startMobileUpload(ctx context.Context, id, generation, dir string, directory, overwrite bool) (model.TransferProgress, error) {
	stage, err := b.newMobileStage(id, generation)
	if err != nil {
		return model.TransferProgress{}, err
	}
	locals, err := stage.Import(ctx, directory)
	if err == nil {
		err = ctx.Err()
	}
	if err != nil {
		return model.TransferProgress{}, closeMobileStage(ctx, stage, err)
	}
	result, err := b.manager.StartUploadPrepared(id, generation, dir, locals, overwrite, stage.Context(), stage.Close)
	if err != nil {
		return result, closeMobileStage(ctx, stage, err)
	}
	return result, nil
}
func (b *Backend) startMobileDownload(ctx context.Context, id, generation string, remotes []string) (model.TransferProgress, error) {
	if len(remotes) == 0 || len(remotes) > 1024 {
		return model.TransferProgress{}, fmt.Errorf("请选择 1..1024 个项目")
	}
	stage, err := b.newMobileStage(id, generation)
	if err != nil {
		return model.TransferProgress{}, err
	}
	err = stage.SelectExport(ctx, "")
	if err == nil {
		err = ctx.Err()
	}
	if err != nil {
		return model.TransferProgress{}, closeMobileStage(ctx, stage, err)
	}
	result, err := b.manager.StartDownloadPrepared(id, generation, remotes, stage.Path(), mobilefiles.MaxBytes, stage.Context(), func(ctx context.Context, progress func(uint64, int)) error {
		return stage.Export(ctx, progress)
	}, stage.Close)
	if err != nil {
		return result, closeMobileStage(ctx, stage, err)
	}
	return result, nil
}
func (b *Backend) mobileUpload(ctx context.Context, id, generation, dir string) error {
	stage, err := b.newMobileStage(id, generation)
	if err != nil {
		return err
	}
	locals, err := stage.Import(ctx, false)
	if err == nil {
		ctx, cancel := context.WithCancel(ctx)
		stop := context.AfterFunc(stage.Context(), cancel)
		_, err = b.manager.UploadBatch(ctx, id, generation, dir, locals, false)
		stop()
		cancel()
	}
	return closeMobileStage(ctx, stage, err)
}
func (b *Backend) mobileDownload(ctx context.Context, id, generation string, remotes []string, name string) (model.TransferSummary, error) {
	if len(remotes) == 0 || len(remotes) > 1024 {
		return model.TransferSummary{}, fmt.Errorf("请选择 1..1024 个项目")
	}
	stage, err := b.newMobileStage(id, generation)
	if err != nil {
		return model.TransferSummary{}, err
	}
	result := model.TransferSummary{Bytes: "0"}
	err = stage.SelectExport(ctx, name)
	if err == nil {
		ctx, cancel := context.WithCancel(ctx)
		stop := context.AfterFunc(stage.Context(), cancel)
		result, err = b.manager.DownloadBatch(session.WithPrivateDownloadStaging(ctx, mobilefiles.MaxBytes), id, generation, remotes, stage.Path())
		if err == nil {
			err = stage.Export(ctx, nil)
		}
		stop()
		cancel()
	}
	return result, closeMobileStage(ctx, stage, err)
}
