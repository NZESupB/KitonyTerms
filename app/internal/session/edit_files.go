package session

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path"

	"github.com/pkg/sftp"
)

const MaxEditBytes = 32 << 20

// 外部编辑仅在Go内传递内容，不通过前端或配置同步。
func readEditContent(client *sftp.Client, target string) ([]byte, os.FileInfo, error) {
	info, err := client.Lstat(target)
	if err != nil {
		return nil, nil, err
	}
	if !info.Mode().IsRegular() || info.Size() < 0 || info.Size() > MaxEditBytes {
		return nil, nil, fmt.Errorf("外部编辑仅支持不超过32 MiB的普通文件")
	}
	file, err := client.Open(target)
	if err != nil {
		return nil, nil, err
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(file, MaxEditBytes+1))
	if err != nil {
		return nil, nil, err
	}
	if len(data) > MaxEditBytes {
		return nil, nil, fmt.Errorf("文件超过32 MiB")
	}
	return data, info, nil
}

func (m *Manager) ReadEditFile(ctx context.Context, id, generation, target string) ([]byte, string, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return nil, "", err
	}
	if err = validRemote(target); err != nil {
		return nil, "", err
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return nil, "", err
	}
	defer release()
	data, _, err := readEditContent(client, target)
	if err != nil {
		return nil, "", err
	}
	return data, digest(data), nil
}

func (m *Manager) WriteEditFile(ctx context.Context, id, generation, target string, data []byte, expected string) (revision string, err error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return "", err
	}
	if err = validRemote(target); err != nil {
		return "", err
	}
	if len(data) > MaxEditBytes || len(expected) != 64 {
		return "", fmt.Errorf("外部编辑内容或版本无效")
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return "", err
	}
	defer release()
	old, info, err := readEditContent(client, target)
	if err != nil {
		return "", err
	}
	if digest(old) != expected {
		return "", fmt.Errorf("远端文件已更改，请保留本地副本并重新读取合并")
	}
	temporary := path.Join(path.Dir(target), ".kitony-edit-"+randomID())
	file, err := client.OpenFile(temporary, os.O_CREATE|os.O_EXCL|os.O_WRONLY)
	if err != nil {
		return "", err
	}
	defer func() { err = errors.Join(err, s.cleanupTemporary(ctx, client, temporary)) }()
	_, writeErr := file.Write(data)
	closeErr := file.Close()
	if writeErr != nil || closeErr != nil {
		return "", errors.Join(writeErr, closeErr)
	}
	if err = client.Chmod(temporary, info.Mode().Perm()); err != nil {
		return "", err
	}
	current, _, err := readEditContent(client, target)
	if err != nil {
		return "", err
	}
	if digest(current) != expected {
		return "", fmt.Errorf("上传期间远端文件已改变，本地副本已保留")
	}
	if err = ctx.Err(); err != nil {
		return "", err
	}
	if err = commitRemote(client, temporary, target, info); err != nil {
		return "", err
	}
	return digest(data), nil
}

func (m *Manager) EditSessionActive(id, generation string) bool {
	s, err := m.Get(id, generation)
	if err != nil {
		return false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.status == "connected" && s.ctx.Err() == nil
}
