package update

import (
	"context"
	"errors"
	"fmt"
	"os"
	"strconv"
	"sync"
	"time"

	"kitonyterms/internal/model"
)

type task struct {
	kind       string
	generation uint64
	ctx        context.Context
	cancel     context.CancelFunc
	done       chan struct{}
}

type Service struct {
	op           sync.Mutex
	mu           sync.Mutex
	root         *os.Root
	desktop      *Desktop
	installation installation
	access       installationAccess
	current      BuildInfo
	client       *releaseClient
	factory      func(model.UpdateProxy) (*releaseClient, error)
	ctx          context.Context
	cancel       context.CancelFunc
	workers      sync.WaitGroup
	closed       bool
	closedDone   chan struct{}
	closeErr     error
	task         *task
	preparation  *servicePreparation
	generation   uint64
	revision     uint64
	sequence     uint64
	state        model.UpdateSnapshot
	available    *available
	nextCheck    time.Time
	notBefore    time.Time
	failures     uint
}

func New(parent context.Context, path string, desktop *Desktop) (*Service, error) {
	if desktop == nil {
		return nil, fmt.Errorf("桌面更新缺少进程锁所有者")
	}
	info, err := CurrentBuild()
	if err != nil {
		return nil, err
	}
	return openService(parent, path, info, desktop, newClient)
}
func openService(parent context.Context, path string, current BuildInfo, desktop *Desktop, factory func(model.UpdateProxy) (*releaseClient, error)) (*Service, error) {
	if current.Channel != "development" {
		if err := current.Validate(); err != nil {
			return nil, err
		}
	}
	root, err := openRoot(path)
	if err != nil {
		return nil, err
	}
	preferences, notice, err := readPreferences(root, current)
	if err != nil {
		root.Close()
		return nil, err
	}
	var install installation
	var access installationAccess
	if desktop != nil {
		install, access, err = desktop.inspectForService(parent, root, current)
		if err != nil {
			return nil, errors.Join(err, root.Close())
		}
	}
	client, err := factory(preferences.Proxy)
	if err != nil {
		root.Close()
		return nil, err
	}
	ctx, cancel := context.WithCancel(parent)
	phase, reason := "idle", ""
	if current.Channel == "development" {
		phase = "disabled"
		reason = "development"
	}
	if current.Target == "" {
		phase = "disabled"
		reason = "unsupported_platform"
	}
	if reason == "" && access.Reason != "" {
		phase, reason = "disabled", access.Reason
	}
	s := &Service{root: root, desktop: desktop, installation: install, access: access, current: current, client: client, factory: factory, ctx: ctx, cancel: cancel, closedDone: make(chan struct{}), revision: uint64(time.Now().UnixNano()), nextCheck: time.Now().Add(5 * time.Second), state: model.UpdateSnapshot{Phase: phase, Current: current.dto(), Preferences: preferences, Notice: notice, DisabledReason: reason, RequiresElevation: install.Installed || access.Elevate, Received: "0", Speed: "0"}}
	s.loadInstallFailure()
	s.sequence = s.revision
	s.workers.Add(1)
	go func() {
		defer s.workers.Done()
		ticker := time.NewTicker(5 * time.Second)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				_ = s.start("check", true, 0)
			}
		}
	}()
	return s, nil
}
func (s *Service) Snapshot() model.UpdateSnapshot {
	s.mu.Lock()
	defer s.mu.Unlock()
	state := s.state
	s.sequence++
	state.Sequence = strconv.FormatUint(s.sequence, 10)
	state.PreferenceRevision = strconv.FormatUint(s.revision, 10)
	state.Busy = s.task != nil || s.preparation != nil
	state.InstallAvailable = !s.closed && s.desktop != nil && s.preparation == nil && s.task == nil && s.available != nil && state.DisabledReason == "" && state.Phase == "downloaded"
	if state.Available != nil {
		copy := *state.Available
		state.Available = &copy
	}
	if state.Failure != nil {
		copy := *state.Failure
		state.Failure = &copy
	}
	next := s.nextCheck
	if s.notBefore.After(next) {
		next = s.notBefore
	}
	if !next.IsZero() {
		state.NextCheck = strconv.FormatInt(next.Unix(), 10)
	}
	return state
}
func (s *Service) Check() error    { return s.start("check", false, 0) }
func (s *Service) Download() error { return s.start("download", false, 0) }
func (s *Service) start(kind string, automatic bool, expected uint64) error {
	s.op.Lock()
	defer s.op.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed || s.ctx.Err() != nil {
		return fmt.Errorf("更新服务已停止")
	}
	if s.preparation != nil {
		if automatic {
			return nil
		}
		return fmt.Errorf("正在准备更新安装")
	}
	if s.state.DisabledReason != "" {
		return fmt.Errorf("此构建未启用更新")
	}
	if s.task != nil {
		if automatic {
			return nil
		}
		return fmt.Errorf("更新任务正在运行")
	}
	now := time.Now()
	if automatic {
		if kind == "check" && (!s.state.Preferences.AutoCheck || s.state.Phase == "downloaded" || now.Before(s.nextCheck)) {
			return nil
		}
		if kind == "download" && (!s.state.Preferences.AutoDownload || s.state.Phase != "available" || s.generation != expected) {
			return nil
		}
	}
	if now.Before(s.notBefore) {
		f := &failure{Kind: "rate_limited", Stage: kind, Retry: s.notBefore.Sub(now)}
		s.state.Phase = "error"
		s.state.Failure = f.dto(now)
		return f
	}
	if kind == "download" && s.available == nil {
		return fmt.Errorf("请先检查更新")
	}
	s.generation++
	ctx, cancel := context.WithCancel(s.ctx)
	t := &task{kind: kind, generation: s.generation, ctx: ctx, cancel: cancel, done: make(chan struct{})}
	s.task = t
	s.state.Failure = nil
	s.state.Speed = "0"
	s.state.Received = "0"
	if kind == "check" {
		s.state.Phase = "checking"
	} else {
		s.state.Phase = "downloading"
	}
	client := s.client
	var candidate available
	if s.available != nil {
		candidate = *s.available
	}
	s.workers.Add(1)
	go s.run(t, client, candidate)
	return nil
}
func (s *Service) run(t *task, client *releaseClient, candidate available) {
	defer s.workers.Done()
	defer t.cancel()
	var next *available
	var err error
	if t.kind == "check" {
		next, err = client.check(t.ctx, s.current, s.installation.Installed)
	} else {
		err = client.download(t.ctx, s.root, candidate, func(received, speed uint64) {
			s.mu.Lock()
			defer s.mu.Unlock()
			if s.task == t && s.generation == t.generation && t.ctx.Err() == nil {
				s.state.Received = strconv.FormatUint(received, 10)
				s.state.Speed = strconv.FormatUint(speed, 10)
			}
		})
	}
	autoDownload := false
	s.mu.Lock()
	if s.task == t {
		s.task = nil
		s.state.Speed = "0"
		if t.ctx.Err() != nil || s.generation != t.generation || s.closed {
			if t.kind == "check" {
				s.state.Phase = "cancelled"
				s.nextCheck = time.Now().Add(6 * time.Hour)
			} else {
				s.state.Phase = "available"
			}
		} else if err != nil {
			f := failureOf(err, t.kind)
			s.state.Phase = "error"
			s.state.Failure = f.dto(time.Now())
			s.failures++
			backoff := 30 * time.Second * time.Duration(1<<min(s.failures, 7))
			if backoff > time.Hour {
				backoff = time.Hour
			}
			if f.Retry > 0 {
				backoff = f.Retry
				s.notBefore = time.Now().Add(backoff)
			}
			s.nextCheck = time.Now().Add(backoff)
			if t.kind == "download" && f.Status == 404 {
				s.available = nil
				s.state.Available = nil
			}
		} else {
			s.failures = 0
			if t.kind == "check" {
				s.available = next
				s.state.Available = nil
				s.state.LastCheck = strconv.FormatInt(time.Now().Unix(), 10)
				s.nextCheck = time.Now().Add(6 * time.Hour)
				s.state.Phase = "idle"
				if next != nil {
					value := next.dto()
					s.state.Available = &value
					s.state.Phase = "available"
					autoDownload = s.state.Preferences.AutoDownload
				}
			} else {
				s.state.Phase = "downloaded"
				s.state.Received = strconv.FormatUint(candidate.Package.Size, 10)
			}
		}
	}
	s.mu.Unlock()
	close(t.done)
	if autoDownload {
		_ = s.start("download", true, t.generation)
	}
}

// 控制操作持有op锁，等待旧任务彻底退出后才准许下一个任务。
func (s *Service) stopTask() {
	s.mu.Lock()
	t := s.task
	s.generation++
	if t != nil {
		s.state.Phase = "cancelling"
		t.cancel()
	}
	s.mu.Unlock()
	if t != nil {
		<-t.done
	}
}
func (s *Service) Cancel() error {
	s.op.Lock()
	defer s.op.Unlock()
	s.mu.Lock()
	closed, preparing := s.closed, s.preparation != nil
	s.mu.Unlock()
	if preparing {
		return fmt.Errorf("请通过安装请求取消准备")
	}
	if closed {
		return fmt.Errorf("更新服务已停止")
	}
	s.stopTask()
	return nil
}
func (s *Service) SavePreferences(ctx context.Context, request model.SaveUpdatePreferencesRequest) (model.UpdateSnapshot, error) {
	s.op.Lock()
	defer s.op.Unlock()
	if err := ctx.Err(); err != nil {
		return model.UpdateSnapshot{}, err
	}
	s.mu.Lock()
	if s.closed || s.ctx.Err() != nil {
		s.mu.Unlock()
		return model.UpdateSnapshot{}, fmt.Errorf("更新服务已停止")
	}
	if s.preparation != nil {
		s.mu.Unlock()
		return model.UpdateSnapshot{}, fmt.Errorf("安装准备期间不能修改更新偏好")
	}
	if request.ExpectedRevision != strconv.FormatUint(s.revision, 10) {
		s.mu.Unlock()
		return model.UpdateSnapshot{}, fmt.Errorf("更新偏好版本冲突，请重新加载")
	}
	changed := request.Preferences.Proxy != s.state.Preferences.Proxy
	client := s.client
	s.mu.Unlock()
	if err := validateProxy(request.Preferences.Proxy); err != nil {
		return model.UpdateSnapshot{}, err
	}
	if changed {
		var err error
		client, err = s.factory(request.Preferences.Proxy)
		if err != nil {
			return model.UpdateSnapshot{}, err
		}
	}
	if err := ctx.Err(); err != nil {
		if changed {
			client.close()
		}
		return model.UpdateSnapshot{}, err
	}
	if err := atomicJSON(s.root, "preferences.json", request.Preferences); err != nil {
		if changed {
			client.close()
		}
		return model.UpdateSnapshot{}, fmt.Errorf("保存更新偏好失败：%w", err)
	}
	// 已提交磁盘后完成对应内存切换，不让请求取消留下两套偏好。
	if changed {
		s.stopTask()
	}
	s.mu.Lock()
	old := s.client
	s.client = client
	s.revision++
	s.state.Preferences = request.Preferences
	s.state.Notice = ""
	if changed {
		s.failures = 0
		s.nextCheck = time.Now()
		s.state.Failure = nil
		if s.state.Phase == "error" || s.state.Phase == "cancelled" {
			if s.available != nil {
				s.state.Phase = "available"
			} else {
				s.state.Phase = "idle"
			}
		}
	}
	s.mu.Unlock()
	if changed {
		old.close()
	}
	return s.Snapshot(), nil
}
func (s *Service) Close() error {
	s.op.Lock()
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		s.op.Unlock()
		<-s.closedDone
		return s.closeErr
	}
	s.closed = true
	s.cancel()
	p := s.preparation
	if p != nil {
		p.cancel()
	}
	s.mu.Unlock()
	s.stopTask()
	var installErr error
	if p != nil {
		<-p.done
		if !p.committed {
			installErr = s.desktop.abortInstall(p.id)
		}
	}
	s.op.Unlock()
	s.workers.Wait()
	s.client.close()
	s.closeErr = errors.Join(installErr, s.root.Close())
	close(s.closedDone)
	return s.closeErr
}
