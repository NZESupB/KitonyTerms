package session

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/pkg/sftp"
	"kitonyterms/internal/model"
)

type uploadNode struct {
	root     *os.Root
	relative string
	info     os.FileInfo
	previous os.FileInfo
}
type uploadManifest struct {
	roots []*os.Root
	nodes []uploadNode
}

func (manifest *uploadManifest) close() {
	for _, root := range manifest.roots {
		_ = root.Close()
	}
}
func scanUpload(ctx context.Context, locals []string) (*uploadManifest, error) {
	manifest := &uploadManifest{}
	ok := false
	defer func() {
		if !ok {
			manifest.close()
		}
	}()
	if len(locals) == 0 || len(locals) > 1024 {
		return nil, fmt.Errorf("请选择 1..1024 个文件或目录")
	}
	seen := map[string]bool{}
	var walk func(*os.Root, string, int) error
	walk = func(root *os.Root, relative string, depth int) error {
		if err := ctx.Err(); err != nil {
			return err
		}
		if depth > 64 || len(manifest.nodes) >= 50000 {
			return fmt.Errorf("目录层级或项目数超过限制")
		}
		for _, name := range strings.Split(relative, "/") {
			if name == "" || name == "." || name == ".." || strings.ContainsAny(name, "\\\x00\r\n") {
				return fmt.Errorf("上传名称无效")
			}
		}
		if err := validRemote(relative); err != nil {
			return err
		}
		key := strings.ToLower(relative)
		if seen[key] {
			return fmt.Errorf("上传目标名称冲突: %s", relative)
		}
		seen[key] = true
		info, err := root.Lstat(filepath.FromSlash(relative))
		if err != nil {
			return err
		}
		if !info.IsDir() && !info.Mode().IsRegular() {
			return fmt.Errorf("不支持链接或特殊文件: %s", relative)
		}
		manifest.nodes = append(manifest.nodes, uploadNode{root: root, relative: relative, info: info})
		if !info.IsDir() {
			return nil
		}
		dir, err := root.Open(filepath.FromSlash(relative))
		if err != nil {
			return err
		}
		defer dir.Close()
		for {
			entries, readErr := dir.ReadDir(256)
			for _, entry := range entries {
				if err := walk(root, path.Join(relative, entry.Name()), depth+1); err != nil {
					return err
				}
			}
			if readErr == io.EOF {
				return nil
			}
			if readErr != nil {
				return readErr
			}
		}
	}
	for _, local := range locals {
		absolute, err := filepath.Abs(local)
		if err != nil {
			return nil, err
		}
		name := filepath.Base(absolute)
		if name == "." || name == string(filepath.Separator) {
			return nil, fmt.Errorf("不能上传本地根目录")
		}
		root, err := os.OpenRoot(filepath.Dir(absolute))
		if err != nil {
			return nil, err
		}
		manifest.roots = append(manifest.roots, root)
		if err := walk(root, name, 0); err != nil {
			return nil, err
		}
	}
	ok = true
	return manifest, nil
}
func sameRemoteFile(a, b os.FileInfo) bool {
	return a.Mode() == b.Mode() && a.Size() == b.Size() && a.ModTime().Equal(b.ModTime())
}

// 所有提交均先写同目录临时文件；旧文件只在可恢复的提交阶段让位。
func commitRemote(client *sftp.Client, temporary, target string, previous os.FileInfo) error {
	current, err := client.Lstat(target)
	if previous == nil {
		if err == nil {
			return fmt.Errorf("传输期间出现同名项目，未覆盖: %s", target)
		}
		if !os.IsNotExist(err) {
			return err
		}
		return client.Rename(temporary, target)
	}
	if err != nil {
		return err
	}
	if !current.Mode().IsRegular() || !sameRemoteFile(previous, current) {
		return fmt.Errorf("传输期间远端文件已改变: %s", target)
	}
	if _, supported := client.HasExtension("posix-rename@openssh.com"); supported {
		return client.PosixRename(temporary, target)
	}
	backup := path.Join(path.Dir(target), ".kitony-backup-"+randomID())
	if err = client.Rename(target, backup); err != nil {
		return err
	}
	if err = client.Rename(temporary, target); err != nil {
		if restoreErr := client.Rename(backup, target); restoreErr != nil {
			return fmt.Errorf("提交失败: %v；恢复失败，备份保留于 %s: %w", err, backup, restoreErr)
		}
		return err
	}
	if err = client.Remove(backup); err != nil {
		return fmt.Errorf("文件已提交，备份清理失败 %s: %w", backup, err)
	}
	return nil
}
func copyLocalFile(ctx context.Context, s *Session, client *sftp.Client, target string, node uploadNode) (count int64, err error) {
	before, err := node.root.Lstat(filepath.FromSlash(node.relative))
	if err != nil {
		return 0, err
	}
	if !before.Mode().IsRegular() || !os.SameFile(node.info, before) || !sameRemoteFile(node.info, before) {
		return 0, fmt.Errorf("本地文件已改变: %s", node.relative)
	}
	source, err := node.root.Open(filepath.FromSlash(node.relative))
	if err != nil {
		return 0, err
	}
	defer source.Close()
	opened, err := source.Stat()
	if err != nil {
		return 0, err
	}
	if !os.SameFile(before, opened) {
		return 0, fmt.Errorf("本地文件在打开时已替换")
	}
	temporary := path.Join(path.Dir(target), ".kitony-upload-"+randomID())
	file, err := client.OpenFile(temporary, os.O_WRONLY|os.O_CREATE|os.O_EXCL)
	if err != nil {
		return 0, err
	}
	defer func() { err = errors.Join(err, s.cleanupTemporary(ctx, client, temporary)) }()
	if err = file.Chmod(0600); err != nil {
		_ = file.Close()
		return 0, err
	}
	count, err = io.Copy(progressWriter{file, transferFrom(ctx)}, contextReader{ctx, io.LimitReader(source, before.Size())})
	if err == nil {
		var extra [1]byte
		n, readErr := source.Read(extra[:])
		if count != before.Size() || n != 0 || readErr != io.EOF {
			err = fmt.Errorf("上传期间本地文件大小已改变")
		}
	}
	if err == nil {
		after, statErr := source.Stat()
		if statErr != nil {
			err = statErr
		} else if !sameRemoteFile(before, after) {
			err = fmt.Errorf("上传期间本地文件已改变")
		}
	}
	if err == nil {
		if _, supported := client.HasExtension("fsync@openssh.com"); supported {
			err = file.Sync()
		}
	}
	closeErr := file.Close()
	if err != nil {
		return 0, err
	}
	if closeErr != nil {
		return 0, closeErr
	}
	mode := node.info.Mode().Perm()
	if node.previous != nil {
		mode = node.previous.Mode().Perm()
	}
	if err = client.Chmod(temporary, mode); err != nil {
		return 0, err
	}
	if err = ctx.Err(); err != nil {
		return 0, err
	}
	if err = commitRemote(client, temporary, target, node.previous); err != nil {
		return 0, err
	}
	return count, nil
}
func (m *Manager) UploadBatch(ctx context.Context, id, generation, dir string, locals []string, overwrite bool) (model.TransferSummary, error) {
	result := model.TransferSummary{Bytes: "0"}
	if err := validRemote(dir); err != nil {
		return result, err
	}
	s, err := m.Get(id, generation)
	if err != nil {
		return result, err
	}
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	stop := context.AfterFunc(s.ctx, cancel)
	defer stop()
	manifest, err := scanUpload(ctx, locals)
	if err != nil {
		return result, err
	}
	defer manifest.close()
	client, release, err := s.fileScope(ctx)
	if err != nil {
		return result, err
	}
	defer release()
	canonical, err := client.RealPath(dir)
	if err != nil {
		return result, err
	}
	parent, err := client.Stat(canonical)
	if err != nil {
		return result, err
	}
	if !parent.IsDir() {
		return result, fmt.Errorf("上传目标不是目录")
	}
	sizes := []int64{}
	for i := range manifest.nodes {
		if err = ctx.Err(); err != nil {
			return result, err
		}
		node := &manifest.nodes[i]
		target := path.Join(canonical, node.relative)
		if err = validRemote(target); err != nil {
			return result, err
		}
		previous, statErr := client.Lstat(target)
		if statErr == nil {
			if !overwrite {
				return result, fmt.Errorf("远端目标已存在，未覆盖: %s", node.relative)
			}
			if previous.IsDir() != node.info.IsDir() || (!previous.IsDir() && !previous.Mode().IsRegular()) {
				return result, fmt.Errorf("远端类型冲突或链接: %s", node.relative)
			}
			node.previous = previous
		} else if !os.IsNotExist(statErr) {
			return result, statErr
		}
		if !node.info.IsDir() {
			sizes = append(sizes, node.info.Size())
		}
	}
	job := transferFrom(ctx)
	job.plan(sizes)
	var total uint64
	for _, node := range manifest.nodes {
		if err = ctx.Err(); err != nil {
			return result, err
		}
		job.current(node.relative)
		target := path.Join(canonical, node.relative)
		if node.info.IsDir() {
			if node.previous == nil {
				err = client.Mkdir(target)
			} else {
				current, statErr := client.Lstat(target)
				err = statErr
				if err == nil && !current.IsDir() {
					err = fmt.Errorf("远端目录类型已改变")
				}
			}
		} else {
			var count int64
			count, err = copyLocalFile(ctx, s, client, target, node)
			if err == nil {
				result.Files++
				total += uint64(count)
				result.Bytes = strconv.FormatUint(total, 10)
				job.completed()
			}
		}
		if err != nil {
			return result, fmt.Errorf("上传停止于 %s，已完成文件保留: %w", node.relative, err)
		}
	}
	return result, nil
}
