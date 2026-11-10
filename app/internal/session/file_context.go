package session

import (
	"context"
	"sync"

	"github.com/pkg/sftp"
)

// 同一会话的文件提交串行执行，等待者仍可取消。
type fileMutex struct {
	once sync.Once
	gate chan struct{}
}

func (m *fileMutex) LockContext(ctx context.Context) error {
	m.once.Do(func() { m.gate = make(chan struct{}, 1) })
	select {
	case m.gate <- struct{}{}:
		if err := ctx.Err(); err != nil {
			m.Unlock()
			return err
		}
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}
func (m *fileMutex) Unlock() { <-m.gate }

func (s *Session) fileScope(ctx context.Context) (*sftp.Client, func(), error) {
	ctx, cancel := context.WithCancel(ctx)
	stopSession := context.AfterFunc(s.ctx, cancel)
	cleanup := func() { stopSession(); cancel() }
	if err := s.fileMu.LockContext(ctx); err != nil {
		cleanup()
		return nil, nil, err
	}
	client, err := s.files(ctx)
	if err != nil {
		cleanup()
		s.fileMu.Unlock()
		return nil, nil, err
	}
	closed := make(chan struct{})
	stop := context.AfterFunc(ctx, func() { _ = client.Close(); close(closed) })
	release := func() {
		if !stop() {
			<-closed
		}
		if ctx.Err() != nil {
			s.mu.Lock()
			if s.sftp == client {
				s.sftp = nil
			}
			s.mu.Unlock()
		}
		cleanup()
		s.fileMu.Unlock()
	}
	if err = ctx.Err(); err != nil {
		release()
		return nil, nil, err
	}
	return client, release, nil
}
