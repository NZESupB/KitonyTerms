package session

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/pkg/sftp"
)

// 取消关闭旧通道后，用独立且限时的 SFTP 通道回收本次创建的临时文件。
func (s *Session) cleanupTemporary(ctx context.Context, client *sftp.Client, temporary string) error {
	err := client.Remove(temporary)
	if err == nil || os.IsNotExist(err) {
		return nil
	}
	if ctx.Err() != nil && s.ctx.Err() == nil {
		s.mu.Lock()
		host := s.client
		s.mu.Unlock()
		if host != nil {
			cleanup, cancel := context.WithTimeout(s.ctx, 2*time.Second)
			defer cancel()
			recovery, openErr := openSFTP(cleanup, host)
			if openErr == nil {
				stop := context.AfterFunc(cleanup, func() { _ = recovery.Close() })
				err = recovery.Remove(temporary)
				stop()
				_ = recovery.Close()
			} else {
				err = openErr
			}
		}
	}
	if err == nil || os.IsNotExist(err) {
		return nil
	}
	return fmt.Errorf("临时文件清理失败，副本保留于 %s: %w", temporary, err)
}
