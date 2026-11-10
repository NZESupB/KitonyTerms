package session

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path"
	"sort"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/pkg/sftp"
	"kitonyterms/internal/model"
)

const maxTextBytes = 1 << 20

func (s *Session) files(ctx context.Context) (*sftp.Client, error) {
	s.mu.Lock()
	if s.client == nil || s.status != "connected" || s.ctx.Err() != nil {
		s.mu.Unlock()
		return nil, fmt.Errorf("SSH 会话未连接")
	}
	if s.sftp != nil {
		client := s.sftp
		s.mu.Unlock()
		return client, nil
	}
	sshClient := s.client
	s.mu.Unlock()
	client, err := openSFTP(ctx, sshClient)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ctx.Err() != nil || s.client != sshClient {
		client.Close()
		return nil, fmt.Errorf("SSH 会话已关闭")
	}
	s.sftp = client
	return client, nil
}
func validRemote(p string) error {
	if p == "" || len(p) > 8192 || strings.ContainsAny(p, "\x00\r\n") {
		return fmt.Errorf("远端路径无效")
	}
	return nil
}
func (m *Manager) ReadDirectory(ctx context.Context, id, generation, dir string) (model.Directory, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return model.Directory{}, err
	}
	if dir == "" {
		dir = "."
	}
	if err = validRemote(dir); err != nil {
		return model.Directory{}, err
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return model.Directory{}, err
	}
	defer release()
	canonical, err := client.RealPath(dir)
	if err != nil {
		return model.Directory{}, err
	}
	entries, err := client.ReadDirContext(ctx, canonical)
	if err != nil {
		return model.Directory{}, err
	}
	if len(entries) > 50000 {
		return model.Directory{}, fmt.Errorf("目录条目过多")
	}
	out := model.Directory{Path: canonical, Entries: []model.FileEntry{}}
	for _, e := range entries {
		owner := "--"
		if attributes, ok := e.Sys().(*sftp.FileStat); ok {
			owner = fmt.Sprintf("%d:%d", attributes.UID, attributes.GID)
		}
		out.Entries = append(out.Entries, model.FileEntry{Name: e.Name(), Path: path.Join(canonical, e.Name()), IsDir: e.IsDir(), IsSymlink: e.Mode()&os.ModeSymlink != 0, Size: fmt.Sprint(e.Size()), Modified: e.ModTime().UTC().Format(time.RFC3339), Mode: e.Mode().String(), OwnerGroup: owner})
	}
	sort.Slice(out.Entries, func(i, j int) bool {
		a, b := out.Entries[i], out.Entries[j]
		if a.IsDir != b.IsDir {
			return a.IsDir
		}
		return strings.ToLower(a.Name) < strings.ToLower(b.Name)
	})
	return out, nil
}
func digest(data []byte) string { h := sha256.Sum256(data); return hex.EncodeToString(h[:]) }
func readText(client *sftp.Client, p string) (model.TextFile, error) {
	info, err := client.Lstat(p)
	if err != nil {
		return model.TextFile{}, err
	}
	if !info.Mode().IsRegular() || info.Size() > maxTextBytes {
		return model.TextFile{}, fmt.Errorf("仅支持不超过 1 MiB 的普通 UTF-8 文件")
	}
	file, err := client.Open(p)
	if err != nil {
		return model.TextFile{}, err
	}
	defer file.Close()
	data, err := io.ReadAll(io.LimitReader(file, maxTextBytes+1))
	if err != nil {
		return model.TextFile{}, err
	}
	if len(data) > maxTextBytes || !utf8.Valid(data) {
		return model.TextFile{}, fmt.Errorf("文件过大或不是 UTF-8")
	}
	return model.TextFile{Path: p, Content: string(data), Revision: digest(data)}, nil
}
func (m *Manager) ReadText(ctx context.Context, id, generation, p string) (model.TextFile, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return model.TextFile{}, err
	}
	if err = validRemote(p); err != nil {
		return model.TextFile{}, err
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return model.TextFile{}, err
	}
	defer release()
	return readText(client, p)
}
func (m *Manager) WriteText(ctx context.Context, id, generation, p, content, expected string) (result model.TextFile, err error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return model.TextFile{}, err
	}
	if err = validRemote(p); err != nil {
		return model.TextFile{}, err
	}
	if len(content) > maxTextBytes || !utf8.ValidString(content) {
		return model.TextFile{}, fmt.Errorf("仅支持不超过 1 MiB 的 UTF-8 文本")
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return model.TextFile{}, err
	}
	defer release()
	old, err := readText(client, p)
	if err != nil {
		return model.TextFile{}, err
	}
	if old.Revision != expected {
		return model.TextFile{}, fmt.Errorf("远端文件已更改，请重新读取后合并")
	}
	info, err := client.Stat(p)
	if err != nil {
		return model.TextFile{}, err
	}
	temp := path.Join(path.Dir(p), ".kitony-edit-"+randomID())
	file, err := client.OpenFile(temp, os.O_CREATE|os.O_EXCL|os.O_WRONLY)
	if err != nil {
		return model.TextFile{}, err
	}
	defer func() { err = errors.Join(err, s.cleanupTemporary(ctx, client, temp)) }()
	_, err = io.WriteString(file, content)
	closeErr := file.Close()
	if err != nil {
		return model.TextFile{}, err
	}
	if closeErr != nil {
		return model.TextFile{}, closeErr
	}
	if err = client.Chmod(temp, info.Mode().Perm()); err != nil {
		return model.TextFile{}, err
	}
	// 再次检查源文件，减少上传期间覆盖外部编辑的风险。
	current, err := readText(client, p)
	if err != nil {
		return model.TextFile{}, err
	}
	if current.Revision != expected {
		return model.TextFile{}, fmt.Errorf("上传期间远端文件已改变")
	}
	if err = commitRemote(client, temp, p, info); err != nil {
		return model.TextFile{}, err
	}
	return model.TextFile{Path: p, Content: content, Revision: digest([]byte(content))}, nil
}
