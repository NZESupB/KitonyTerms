package ipc

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"kitonyterms/internal/update"
)

func (b *Backend) beginProtectedOperation() (func(), error) {
	b.mu.RLock()
	defer b.mu.RUnlock()
	if !b.ready {
		return nil, fmt.Errorf("应用服务尚未就绪")
	}
	return b.quit.BeginOperation()
}

func (b *Backend) installSafety() error {
	state, err := b.GetQuitState()
	if err != nil {
		return err
	}
	if state.ExternalEdits != 0 || state.Activity.Transfers != 0 || state.Sharing || state.ProtectedOperations != 0 {
		return fmt.Errorf("仍有外部编辑、文件操作或配置同步，请结束后再安装")
	}
	return nil
}

// 准备返回后，页面仍须复核局部编辑和对话框状态，才能提交同一请求。
func (b *Backend) PrepareUpdateInstall(ctx context.Context) (string, error) {
	service, err := b.updateService()
	if err != nil {
		return "", err
	}
	request, err := b.quit.StartInstall(ctx)
	if err != nil {
		return "", err
	}
	defer request.Prepared()
	id := request.ID
	b.app.Event.Emit("app:close-requested")
	if err = b.installSafety(); err == nil {
		err = service.PrepareInstall(request.Context, id)
	}
	if err == nil && !b.quit.ReadyInstall(id) {
		err = fmt.Errorf("安装准备已取消")
	}
	if err != nil {
		abortErr := service.AbortInstall(id)
		b.quit.FailInstall(id)
		b.app.Event.Emit("app:close-requested")
		return "", errors.Join(err, abortErr)
	}
	b.app.Event.Emit("app:close-requested")
	return id, nil
}

func (b *Backend) AbortUpdateInstall(requestID string) error {
	service, err := b.updateServiceForLifecycle()
	if err != nil {
		return err
	}
	request, err := b.quit.BeginInstallAbort(requestID)
	if err != nil {
		return err
	}
	request.WaitPrepared()
	err = service.AbortInstall(requestID)
	b.quit.FailInstall(requestID)
	b.app.Event.Emit("app:close-requested")
	return err
}

func (b *Backend) CommitUpdateInstall(ctx context.Context, requestID string) (err error) {
	service, err := b.updateServiceForLifecycle()
	if err != nil {
		return err
	}
	begin, err := b.quit.BeginInstallCommit(requestID)
	if err != nil || !begin {
		return err
	}
	b.app.Event.Emit("app:close-requested")
	var release func()
	defer func() {
		if err != nil {
			abortErr := service.AbortInstall(requestID)
			if release != nil {
				release()
			}
			b.quit.FailInstall(requestID)
			b.app.Event.Emit("app:close-requested")
			err = errors.Join(err, abortErr)
		}
	}()
	if err = b.installSafety(); err != nil {
		return err
	}
	drainCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	release, err = b.manager.Drain(drainCtx)
	cancel()
	if err != nil {
		return err
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	if err = b.quit.AuthorizeInstall(requestID, func() error { return service.CommitInstall(ctx, requestID) }); err != nil {
		return err
	}
	// 此后只能退出：服务永久关闭交由 Wails shutdown，Desktop 的锁仍由入口持有。
	go b.app.Quit()
	return nil
}

func (b *Backend) updateServiceForLifecycle() (*update.Service, error) {
	b.mu.RLock()
	defer b.mu.RUnlock()
	if !b.ready || b.updates == nil || application.System.IsMobile() {
		return nil, fmt.Errorf("此平台的桌面更新服务尚未就绪")
	}
	return b.updates, nil
}
