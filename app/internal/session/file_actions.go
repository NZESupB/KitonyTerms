package session

import (
	"context"
	"fmt"
	"os"
	"path"
	"strings"

	"github.com/pkg/sftp"
)

func mutableRemote(value string) error {
	if err := validRemote(value); err != nil {
		return err
	}
	clean := path.Clean(value)
	if clean == "/" || clean == "." || clean == ".." {
		return fmt.Errorf("不能修改根目录或当前目录")
	}
	return nil
}

func (m *Manager) CreateDirectory(ctx context.Context, id, generation, target string) error {
	if err := mutableRemote(target); err != nil {
		return err
	}
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return err
	}
	defer release()
	return client.Mkdir(target)
}

func (m *Manager) RenamePath(ctx context.Context, id, generation, source, destination string) error {
	if err := mutableRemote(source); err != nil {
		return err
	}
	if err := mutableRemote(destination); err != nil {
		return err
	}
	if path.Clean(source) == path.Clean(destination) {
		return fmt.Errorf("源路径与目标路径相同")
	}
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return err
	}
	defer release()
	if _, err = client.Lstat(destination); err == nil {
		return fmt.Errorf("目标已存在，未覆盖")
	}
	if !os.IsNotExist(err) {
		return err
	}
	// 标准 SFTP rename 不替换已存在目标；禁止用 posix-rename 覆盖。
	return client.Rename(source, destination)
}

type remoteNode struct {
	remote   string
	relative string
	info     os.FileInfo
}

// 先建立有界清单，拒绝链接和特殊文件，再执行目录下载或递归删除。
func scanRemote(ctx context.Context, client *sftp.Client, roots []string) ([]remoteNode, error) {
	if len(roots) == 0 || len(roots) > 1024 {
		return nil, fmt.Errorf("请选择 1..1024 个项目")
	}
	nodes := make([]remoteNode, 0)
	seen := map[string]bool{}
	var walk func(string, string, int) error
	walk = func(remote, relative string, depth int) error {
		if err := ctx.Err(); err != nil {
			return err
		}
		if depth > 64 || len(nodes) >= 50000 {
			return fmt.Errorf("目录层级或项目数超过限制")
		}
		info, err := client.Lstat(remote)
		if err != nil {
			return err
		}
		if !info.IsDir() && !info.Mode().IsRegular() {
			return fmt.Errorf("不支持链接或特殊文件: %s", remote)
		}
		nodes = append(nodes, remoteNode{remote, relative, info})
		if !info.IsDir() {
			return nil
		}
		entries, err := client.ReadDirContext(ctx, remote)
		if err != nil {
			return err
		}
		for _, entry := range entries {
			name := entry.Name()
			if name == "." || name == ".." || strings.ContainsAny(name, "/\\\x00") {
				return fmt.Errorf("远端目录返回无效名称")
			}
			if err = walk(path.Join(remote, name), path.Join(relative, name), depth+1); err != nil {
				return err
			}
		}
		return nil
	}
	for _, root := range roots {
		if err := mutableRemote(root); err != nil {
			return nil, err
		}
		clean := path.Clean(root)
		if seen[clean] {
			return nil, fmt.Errorf("重复的远端路径")
		}
		for previous := range seen {
			if strings.HasPrefix(clean, previous+"/") || strings.HasPrefix(previous, clean+"/") {
				return nil, fmt.Errorf("选中路径存在父子重叠")
			}
		}
		seen[clean] = true
		if err := walk(clean, path.Base(clean), 0); err != nil {
			return nil, err
		}
	}
	return nodes, nil
}

func (m *Manager) RemovePaths(ctx context.Context, id, generation string, targets []string) error {
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return err
	}
	defer release()
	nodes, err := scanRemote(ctx, client, targets)
	if err != nil {
		return err
	}
	for i := len(nodes) - 1; i >= 0; i-- {
		if err = ctx.Err(); err != nil {
			return err
		}
		node := nodes[i]
		if node.info.IsDir() {
			err = client.RemoveDirectory(node.remote)
		} else {
			err = client.Remove(node.remote)
		}
		if err != nil {
			return fmt.Errorf("删除停止于 %s，已删除项目不会恢复: %w", node.remote, err)
		}
	}
	return nil
}
