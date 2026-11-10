package ipc

import (
	"context"
	"fmt"
	"time"

	"kitonyterms/internal/model"
	"kitonyterms/internal/syncconfig"
)

func (b *Backend) ExportConfig() (syncconfig.Envelope, error) {
	if err := b.check(); err != nil {
		return syncconfig.Envelope{}, err
	}
	return syncconfig.NewEnvelope(b.repo.Snapshot().Config), nil
}
func (b *Backend) ImportConfig(ctx context.Context, expected string, config model.Config) (model.ConfigSnapshot, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.ConfigSnapshot{}, err
	}
	defer done()
	snapshot, err := b.repo.ImportConfig(ctx, expected, config)
	if err == nil {
		b.app.Event.Emit("config:changed", snapshot)
	}
	return snapshot, err
}
func (b *Backend) DownloadWebDAV(ctx context.Context, endpoint syncconfig.Endpoint) (syncconfig.RemoteConfig, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return syncconfig.RemoteConfig{}, err
	}
	defer done()
	return syncconfig.DownloadWebDAV(ctx, endpoint)
}
func (b *Backend) UploadWebDAV(ctx context.Context, endpoint syncconfig.Endpoint, etag string, createOnly bool) (string, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return "", err
	}
	defer done()
	return syncconfig.UploadWebDAV(ctx, endpoint, syncconfig.NewEnvelope(b.repo.Snapshot().Config), etag, createOnly)
}
func (b *Backend) StartShare(bindIP string) (syncconfig.ShareInfo, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return syncconfig.ShareInfo{}, err
	}
	defer done()
	b.shareMu.Lock()
	defer b.shareMu.Unlock()
	if b.share != nil && !b.share.Finished() {
		return syncconfig.ShareInfo{}, fmt.Errorf("已有分享正在等待接收，请先停止")
	}
	share, err := syncconfig.StartShare(b.ctx, b.repo.Snapshot().Config, bindIP, 10*time.Minute)
	if err != nil {
		return syncconfig.ShareInfo{}, err
	}
	b.share = share
	return share.Info(), nil
}
func (b *Backend) GetShare() (*syncconfig.ShareInfo, error) {
	b.shareMu.Lock()
	defer b.shareMu.Unlock()
	if err := b.check(); err != nil {
		return nil, err
	}
	if b.share == nil || b.share.Finished() {
		return nil, nil
	}
	info := b.share.Info()
	return &info, nil
}
func (b *Backend) stopShare() {
	b.shareMu.Lock()
	defer b.shareMu.Unlock()
	if b.share != nil {
		b.share.Stop()
		b.share = nil
	}
}
func (b *Backend) StopShare() error {
	if err := b.check(); err != nil {
		return err
	}
	b.stopShare()
	return nil
}
func (b *Backend) ImportLAN(ctx context.Context, address, code, expected string) (model.ConfigSnapshot, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.ConfigSnapshot{}, err
	}
	defer done()
	var snapshot model.ConfigSnapshot
	err = syncconfig.ImportLAN(ctx, address, code, func(config model.Config) error {
		var err error
		snapshot, err = b.ImportConfig(ctx, expected, config)
		return err
	})
	return snapshot, err
}
