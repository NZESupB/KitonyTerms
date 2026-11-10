package ipc

import (
	"context"
	"fmt"

	"kitonyterms/internal/model"
	"kitonyterms/internal/update"
)

func (b *Backend) updateService() (*update.Service, error) {
	if err := b.check(); err != nil {
		return nil, err
	}
	b.mu.RLock()
	service := b.updates
	b.mu.RUnlock()
	if service == nil {
		return nil, fmt.Errorf("此平台不支持桌面更新")
	}
	return service, nil
}

func (b *Backend) GetUpdateState() (model.UpdateSnapshot, error) {
	service, err := b.updateServiceForLifecycle()
	if err != nil {
		return model.UpdateSnapshot{}, err
	}
	return service.Snapshot(), nil
}

func (b *Backend) SaveUpdatePreferences(ctx context.Context, request model.SaveUpdatePreferencesRequest) (model.UpdateSnapshot, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.UpdateSnapshot{}, err
	}
	defer done()
	service, err := b.updateService()
	if err != nil {
		return model.UpdateSnapshot{}, err
	}
	return service.SavePreferences(ctx, request)
}

func (b *Backend) CheckUpdates() (model.UpdateSnapshot, error) {
	service, err := b.updateService()
	if err != nil {
		return model.UpdateSnapshot{}, err
	}
	if err = service.Check(); err != nil {
		return model.UpdateSnapshot{}, err
	}
	return service.Snapshot(), nil
}

func (b *Backend) DownloadUpdate() (model.UpdateSnapshot, error) {
	service, err := b.updateService()
	if err != nil {
		return model.UpdateSnapshot{}, err
	}
	if err = service.Download(); err != nil {
		return model.UpdateSnapshot{}, err
	}
	return service.Snapshot(), nil
}

func (b *Backend) CancelUpdate() (model.UpdateSnapshot, error) {
	service, err := b.updateService()
	if err != nil {
		return model.UpdateSnapshot{}, err
	}
	if err = service.Cancel(); err != nil {
		return model.UpdateSnapshot{}, err
	}
	return service.Snapshot(), nil
}
