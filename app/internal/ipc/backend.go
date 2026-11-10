package ipc

import (
	"context"
	"errors"
	"fmt"
	"path"
	"path/filepath"
	"runtime"
	"sync"
	"time"

	"github.com/gofrs/flock"
	"github.com/wailsapp/wails/v3/pkg/application"
	"kitonyterms/internal/externaledit"
	"kitonyterms/internal/lifecycle"
	"kitonyterms/internal/mobilefiles"
	"kitonyterms/internal/model"
	"kitonyterms/internal/platform"
	"kitonyterms/internal/session"
	"kitonyterms/internal/storage"
	"kitonyterms/internal/syncconfig"
	"kitonyterms/internal/update"
)

type Backend struct {
	mu          sync.RWMutex
	app         *application.App
	window      application.Window
	quit        *lifecycle.Controller
	paths       platform.Paths
	repo        *storage.Repository
	vault       *storage.Vault
	manager     *session.Manager
	edits       *externaledit.Manager
	mobileFiles *mobilefiles.Manager
	updates     *update.Service
	desktop     *update.Desktop
	lock        *flock.Flock
	ready       bool
	ctx         context.Context
	shareMu     sync.Mutex
	clipboardMu sync.Mutex
	share       *syncconfig.Share
	scannerMu   sync.Mutex
	lastScan    time.Time
}

func init() { application.RegisterEvent[model.ConfigSnapshot]("config:changed") }
func NewBackend(app *application.App, window application.Window, quit *lifecycle.Controller, paths platform.Paths, desktop *update.Desktop) *Backend {
	return &Backend{app: app, window: window, quit: quit, paths: paths, desktop: desktop}
}
func (b *Backend) ServiceStartup(ctx context.Context, _ application.ServiceOptions) error {
	paths := b.paths
	var err error
	if paths.Data == "" {
		paths, err = platform.Discover()
		if err != nil {
			return err
		}
	}
	lock := flock.New(paths.Data + "/kitonyterms.lock")
	if b.desktop == nil {
		locked, lockErr := lock.TryLock()
		if lockErr != nil {
			return lockErr
		}
		if !locked {
			return fmt.Errorf("该数据目录已由另一个 KitonyTerms 进程使用")
		}
	}
	repo, err := storage.OpenRepository(paths.Config, paths.Data)
	if err != nil {
		lock.Close()
		return err
	}
	vault, err := storage.OpenVault(paths.Data)
	if err != nil {
		lock.Close()
		return err
	}
	var updates *update.Service
	if !application.System.IsMobile() {
		updates, err = update.New(ctx, filepath.Join(paths.Data, "updates"), b.desktop)
		if err != nil {
			vault.Close()
			lock.Close()
			return err
		}
	}
	manager := session.New(ctx, repo, vault, func() { b.app.Event.Emit("sessions:changed") })
	edits, err := externaledit.New(ctx, filepath.Join(paths.Data, "sftp-edit"), manager, func(file, command string) error {
		return externaledit.Launch(file, command, b.app.Browser.OpenFile)
	})
	if err != nil {
		if updates != nil {
			err = errors.Join(err, updates.Close())
		}
		manager.Shutdown()
		vault.Close()
		lock.Close()
		return err
	}
	var mobileFiles *mobilefiles.Manager
	if mobilefiles.Available() {
		mobileFiles, err = mobilefiles.Open(ctx, filepath.Join(paths.Data, "mobile-files"))
		if err != nil {
			edits.Close()
			manager.Shutdown()
			if updates != nil {
				err = errors.Join(err, updates.Close())
			}
			vault.Close()
			return errors.Join(err, lock.Close())
		}
	}
	b.mu.Lock()
	b.ctx = ctx
	b.paths = paths
	b.lock = lock
	b.repo = repo
	b.vault = vault
	b.manager = manager
	b.edits = edits
	b.mobileFiles = mobileFiles
	b.updates = updates
	b.ready = true
	b.mu.Unlock()
	if !application.System.IsMobile() {
		b.quit.Enable()
	}
	return nil
}
func (b *Backend) ServiceShutdown() error {
	b.mu.Lock()
	b.ready = false
	manager := b.manager
	edits := b.edits
	vault := b.vault
	lock := b.lock
	updates := b.updates
	mobileFiles := b.mobileFiles
	b.mu.Unlock()
	b.stopShare()
	var updateErr error
	if updates != nil {
		updateErr = updates.Close()
	}
	if edits != nil {
		edits.Close()
	}
	if mobileFiles != nil {
		mobileFiles.Cancel()
	}
	if manager != nil {
		manager.Shutdown()
	}
	if mobileFiles != nil {
		updateErr = errors.Join(updateErr, mobileFiles.Close())
	}
	if vault != nil {
		vault.Close()
	}
	if lock != nil {
		return errors.Join(updateErr, lock.Close())
	}
	return updateErr
}
func (b *Backend) check() error {
	b.mu.RLock()
	defer b.mu.RUnlock()
	if !b.ready {
		return fmt.Errorf("应用服务尚未就绪")
	}
	if b.quit.Snapshot().Preparing {
		return fmt.Errorf("应用正在退出")
	}
	return nil
}
func (b *Backend) Info() (model.AppInfo, error) {
	if err := b.check(); err != nil {
		return model.AppInfo{}, err
	}
	info := model.AppInfo{Platform: runtime.GOOS, Mobile: application.System.IsMobile(), DataDirectory: b.paths.Data, Capabilities: []string{"profiles", "vault-v1", "host-key-confirmation", "ssh-terminal", "sftp-batch-transfers", "typed-operations", "operation-sudo", "container-terminal", "ssh-monitor", "webdav-etag", "lan-v2"}, Limitations: []string{"迁移开发版，尚未接管原应用数据与发布入口", "系统认证与桌面安装仍需原生验收", "手机后台连接、文件导出、扫码及真机 IME 尚未验收", "Windows agent 管道仍需原生验收"}}
	if b.mobileFiles != nil {
		info.Capabilities = append(info.Capabilities, "mobile_files")
	}
	return info, nil
}
func (b *Backend) GetConfig() (model.ConfigSnapshot, error) {
	if err := b.check(); err != nil {
		return model.ConfigSnapshot{}, err
	}
	return b.repo.Snapshot(), nil
}
func (b *Backend) SaveProfile(ctx context.Context, req model.SaveProfileRequest) (model.ConfigSnapshot, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.ConfigSnapshot{}, err
	}
	defer done()
	next, err := b.repo.SaveProfile(ctx, req)
	if err == nil {
		b.app.Event.Emit("config:changed", next)
	}
	return next, err
}
func (b *Backend) DeleteProfile(ctx context.Context, name, expected string) (model.ConfigSnapshot, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.ConfigSnapshot{}, err
	}
	defer done()
	next, err := b.repo.DeleteProfile(ctx, name, expected)
	if err == nil {
		b.app.Event.Emit("config:changed", next)
	}
	return next, err
}
func (b *Backend) SaveSettings(ctx context.Context, req model.SaveSettingsRequest) (model.ConfigSnapshot, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.ConfigSnapshot{}, err
	}
	defer done()
	next, err := b.repo.SaveSettings(ctx, req)
	if err == nil {
		b.app.Event.Emit("config:changed", next)
	}
	return next, err
}
func (b *Backend) SaveSecret(profileName, secret string) error {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return err
	}
	defer done()
	if len(secret) > 65536 {
		return fmt.Errorf("凭据长度超限")
	}
	p, err := b.repo.Profile(profileName)
	if err != nil {
		return err
	}
	return b.vault.Set(p.SecretID(), secret)
}
func (b *Backend) Connect(profileName string) (model.SessionSnapshot, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.SessionSnapshot{}, err
	}
	defer done()
	return b.manager.Connect(profileName)
}
func (b *Backend) GetSessions() ([]model.SessionSnapshot, error) {
	if err := b.check(); err != nil {
		return nil, err
	}
	return b.manager.Snapshots(), nil
}
func (b *Backend) CloseSession(id, generation string) error {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return err
	}
	defer done()
	if err := b.manager.CloseSession(id, generation); err != nil {
		return err
	}
	b.edits.StopSession(id, generation)
	return nil
}
func (b *Backend) AnswerChallenge(answer model.ChallengeAnswer) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.Answer(answer)
}
func (b *Backend) PollTerminal(id, generation, after string) (model.TerminalBatch, error) {
	if err := b.check(); err != nil {
		return model.TerminalBatch{}, err
	}
	return b.manager.Poll(id, generation, after)
}
func (b *Backend) GetTerminalState(id, generation string) (model.TerminalState, error) {
	if err := b.check(); err != nil {
		return model.TerminalState{}, err
	}
	return b.manager.TerminalState(id, generation)
}
func (b *Backend) SendInput(req model.InputRequest) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.Input(req)
}
func (b *Backend) Resize(id, generation string, cols, rows int) error {
	if err := b.check(); err != nil {
		return err
	}
	return b.manager.Resize(id, generation, cols, rows)
}
func (b *Backend) ReadDirectory(ctx context.Context, id, generation, path string) (model.Directory, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.Directory{}, err
	}
	defer done()
	return b.manager.ReadDirectory(ctx, id, generation, path)
}
func (b *Backend) ReadText(ctx context.Context, id, generation, path string) (model.TextFile, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.TextFile{}, err
	}
	defer done()
	return b.manager.ReadText(ctx, id, generation, path)
}
func (b *Backend) WriteText(ctx context.Context, id, generation, path, content, revision string) (model.TextFile, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.TextFile{}, err
	}
	defer done()
	return b.manager.WriteText(ctx, id, generation, path, content, revision)
}
func (b *Backend) RunOperation(ctx context.Context, id, generation, kind, target string) (model.OperationResult, error) {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return model.OperationResult{}, err
	}
	defer done()
	return b.manager.RunOperation(ctx, id, generation, kind, target)
}
func (b *Backend) Download(ctx context.Context, id, generation, path string) error {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return err
	}
	defer done()
	if application.System.IsMobile() {
		_, err := b.mobileDownload(ctx, id, generation, []string{path}, remoteBase(path))
		return err
	}
	local, err := b.app.Dialog.SaveFile().AttachToWindow(b.window).SetFilename(remoteBase(path)).PromptForSingleSelection()
	if err != nil {
		return err
	}
	if local == "" {
		return nil
	}
	return b.manager.Download(ctx, id, generation, path, local)
}
func (b *Backend) Upload(ctx context.Context, id, generation, dir string) error {
	done, err := b.beginProtectedOperation()
	if err != nil {
		return err
	}
	defer done()
	if application.System.IsMobile() {
		return b.mobileUpload(ctx, id, generation, dir)
	}
	local, err := b.app.Dialog.OpenFile().AttachToWindow(b.window).PromptForSingleSelection()
	if err != nil {
		return err
	}
	if local == "" {
		return nil
	}
	return b.manager.Upload(ctx, id, generation, dir, local)
}

func remoteBase(remote string) string { return path.Base(remote) }
