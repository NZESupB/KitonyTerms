package session

import (
	"context"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"kitonyterms/internal/storage"
)

type contextReader struct {
	ctx    context.Context
	reader io.Reader
}

func (r contextReader) Read(data []byte) (int, error) {
	if err := r.ctx.Err(); err != nil {
		return 0, err
	}
	return r.reader.Read(data)
}
func (m *Manager) Download(ctx context.Context, id, generation, remote, local string) error {
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	if err = validRemote(remote); err != nil {
		return err
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return err
	}
	defer release()
	info, err := client.Lstat(remote)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() {
		return fmt.Errorf("当前下载入口仅支持普通文件")
	}
	file, err := client.Open(remote)
	if err != nil {
		return err
	}
	defer file.Close()
	stop := context.AfterFunc(ctx, func() { file.Close() })
	defer stop()
	temp, err := os.CreateTemp(filepath.Dir(local), ".kitony-download-*")
	if err != nil {
		return err
	}
	temporary := temp.Name()
	defer os.Remove(temporary)
	_, err = io.Copy(temp, contextReader{ctx, file})
	if err == nil {
		err = temp.Sync()
	}
	closeErr := temp.Close()
	if err != nil {
		return err
	}
	if closeErr != nil {
		return closeErr
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	return storage.CommitFile(temporary, local)
}
func (m *Manager) Upload(ctx context.Context, id, generation, dir, local string) error {
	_, err := m.UploadBatch(ctx, id, generation, dir, []string{local}, false)
	return err
}
