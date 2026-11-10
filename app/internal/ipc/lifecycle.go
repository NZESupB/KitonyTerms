package ipc

import (
	"context"
	"fmt"

	"github.com/wailsapp/wails/v3/pkg/application"
	"kitonyterms/internal/model"
)

func (b *Backend) GetQuitState() (model.QuitState, error) {
	b.mu.RLock()
	ready, manager := b.ready, b.manager
	b.mu.RUnlock()
	if !ready {
		return model.QuitState{}, fmt.Errorf("应用服务尚未就绪")
	}
	state := b.quit.Snapshot()
	result := model.QuitState{RequestID: state.RequestID, Intent: state.Intent, Preparing: state.Preparing, Ready: state.Ready, Committing: state.Committing, Cancelling: state.Cancelling, ProtectedOperations: state.ProtectedOperations, Activity: manager.Activity()}
	result.ExternalEdits = b.edits.Active()
	b.shareMu.Lock()
	result.Sharing = b.share != nil && !b.share.Finished()
	b.shareMu.Unlock()
	return result, nil
}
func (b *Backend) RequestQuit() (model.QuitState, error) {
	b.mu.RLock()
	ready := b.ready
	b.mu.RUnlock()
	if !ready {
		return model.QuitState{}, fmt.Errorf("应用服务尚未就绪")
	}
	if application.System.IsMobile() {
		return model.QuitState{}, fmt.Errorf("移动端由系统管理应用生命周期")
	}
	b.quit.ShouldQuit()
	return b.GetQuitState()
}
func (b *Backend) CancelQuit(requestID string) error {
	if err := b.check(); err != nil {
		return err
	}
	if err := b.quit.Cancel(requestID); err != nil {
		return err
	}
	b.app.Event.Emit("app:close-requested")
	return nil
}
func (b *Backend) ConfirmQuit(requestID string) error {
	if err := b.check(); err != nil {
		return err
	}
	begin, err := b.quit.Begin(requestID)
	if err != nil || !begin {
		return err
	}
	b.app.Event.Emit("app:close-requested")
	go func() {
		b.stopShare()
		b.edits.Close()
		b.manager.Shutdown()
		if b.quit.Complete(requestID) {
			b.app.Quit()
		}
	}()
	return nil
}

// 仅由完成初始业务加载后的页面调用；普通启动没有待确认事务时幂等返回。
func (b *Backend) ConfirmUpdateStartup(ctx context.Context) error {
	if err := b.check(); err != nil {
		return err
	}
	if b.desktop == nil {
		return fmt.Errorf("当前平台没有桌面更新启动确认")
	}
	return b.desktop.ConfirmReady(ctx)
}
