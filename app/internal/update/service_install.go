package update

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"time"
)

type servicePreparation struct {
	id        string
	cancel    context.CancelFunc
	done      chan struct{}
	err       error
	committed bool
}

// 安装准备不持有 op 等待；取消和关闭可先取消 context，再等待真实准备工作结束。
func (s *Service) PrepareInstall(ctx context.Context, id string) error {
	s.op.Lock()
	s.mu.Lock()
	if id == "" || s.closed || s.ctx.Err() != nil || s.desktop == nil || s.task != nil || s.preparation != nil || s.available == nil || s.state.Phase != "downloaded" || s.state.DisabledReason != "" {
		s.mu.Unlock()
		s.op.Unlock()
		return fmt.Errorf("当前没有可安装的已下载更新")
	}
	prepareCtx, cancel := context.WithTimeout(s.ctx, 2*time.Minute)
	stop := context.AfterFunc(ctx, cancel)
	p := &servicePreparation{id: id, cancel: cancel, done: make(chan struct{})}
	s.preparation = p
	s.state.Phase = "install_preparing"
	s.state.Failure = nil
	candidate, install := *s.available, s.installation
	s.workers.Add(1)
	s.mu.Unlock()
	s.op.Unlock()
	defer s.workers.Done()
	defer stop()
	defer cancel()
	err := s.desktop.prepareInstall(prepareCtx, id, install, candidate)
	s.mu.Lock()
	p.err = err
	if s.preparation == p && !s.closed {
		if err == nil {
			s.state.Phase = "install_ready"
		} else {
			s.state.Phase = "downloaded"
			s.state.Failure = (&failure{Kind: "install_prepare", Stage: "install"}).dto(time.Now())
		}
	}
	s.mu.Unlock()
	close(p.done)
	return err
}

func (s *Service) CommitInstall(ctx context.Context, id string) error {
	s.op.Lock()
	defer s.op.Unlock()
	s.mu.Lock()
	p := s.preparation
	if s.closed || p == nil || id == "" || p.id != id {
		s.mu.Unlock()
		return fmt.Errorf("安装准备请求已失效")
	}
	select {
	case <-p.done:
	default:
		s.mu.Unlock()
		return fmt.Errorf("安装辅助程序尚未准备完成")
	}
	if p.err != nil {
		s.mu.Unlock()
		return fmt.Errorf("安装准备失败，请重新准备")
	}
	if p.committed {
		s.mu.Unlock()
		return nil
	}
	s.mu.Unlock()
	if err := s.desktop.commitInstall(ctx, id); err != nil {
		return err
	}
	s.mu.Lock()
	p.committed = true
	s.state.Phase = "install_committed"
	s.mu.Unlock()
	return nil
}

func (s *Service) AbortInstall(id string) error {
	s.op.Lock()
	defer s.op.Unlock()
	s.mu.Lock()
	p := s.preparation
	if p == nil {
		s.mu.Unlock()
		return nil
	}
	if id == "" || p.id != id || p.committed {
		s.mu.Unlock()
		return fmt.Errorf("安装准备请求已失效或已经提交")
	}
	p.cancel()
	s.mu.Unlock()
	<-p.done
	err := s.desktop.abortInstall(id)
	s.mu.Lock()
	if s.preparation == p {
		s.preparation = nil
		s.state.Phase = "downloaded"
		if err != nil {
			s.state.Phase = "disabled"
			s.state.DisabledReason = "pending_installation"
			s.state.Failure = (&failure{Kind: "install_cleanup", Stage: "install"}).dto(time.Now())
		}
	}
	s.mu.Unlock()
	return err
}

// error.json 沿用旧更新器格式；只展示类别，原始内容留在本机事务目录。
func (s *Service) loadInstallFailure() {
	data, err := readPrivate(s.root, "error.json", metadataLimit)
	if errors.Is(err, fs.ErrNotExist) {
		return
	}
	kind := "install_failed"
	var message string
	if err != nil || decodeJSON(data, &message, true) != nil || message == "" {
		kind = "install_record"
	}
	s.state.Failure = (&failure{Kind: kind, Stage: "install"}).dto(time.Now())
	// 不覆盖独立的代理偏好 Notice。
}
