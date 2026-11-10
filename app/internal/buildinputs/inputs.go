// Package buildinputs 为独立候选构建复制有界、私有的源码输入，不接触产品数据。
package buildinputs

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"reflect"
	"strings"
)

const MaxBytes = 64 << 20
const MaxFileBytes = 16 << 20
const MaxNodes = 5000
const MaxDepth = 64

// Overlap 检查已规范化路径的祖先关系。
func Overlap(first, second string) bool {
	inside := func(parent, child string) bool {
		relative, err := filepath.Rel(parent, child)
		return err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator))
	}
	return inside(first, second) || inside(second, first)
}

func resourceNames(resources []string) ([]string, error) {
	names := []string{"go.mod", "go.sum"}
	seen := map[string]bool{"go.mod": true, "go.sum": true}
	for _, name := range resources {
		if !fs.ValidPath(name) || !strings.HasPrefix(name, "build/") || strings.Contains(name, "\\") || seen[name] {
			return nil, errors.New("额外构建资源必须是唯一的build下相对文件路径")
		}
		seen[name] = true
		names = append(names, name)
	}
	return names, nil
}

// Copy 仅复制Go/native源码、全部嵌入前端成品和调用方固定资源；失败副本保留。
func Copy(ctx context.Context, project, target string, resources []string) (map[string]string, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if _, err := resourceNames(resources); err != nil {
		return nil, err
	}
	if !filepath.IsAbs(project) || !filepath.IsAbs(target) {
		return nil, errors.New("构建输入和副本必须使用绝对路径")
	}
	canonical, err := filepath.EvalSymlinks(project)
	if err != nil {
		return nil, err
	}
	parent, err := filepath.EvalSymlinks(filepath.Dir(filepath.Clean(target)))
	if err != nil {
		return nil, err
	}
	target = filepath.Join(parent, filepath.Base(filepath.Clean(target)))
	if Overlap(canonical, target) {
		return nil, errors.New("隔离构建目录不能与原工程重叠")
	}
	if err = os.Mkdir(target, 0700); err != nil {
		return nil, err
	}
	destination, err := os.OpenRoot(target)
	if err != nil {
		return nil, err
	}
	inputs, scanErr := Scan(ctx, project, resources, func(name string, data []byte) error {
		if err := destination.MkdirAll(filepath.Dir(name), 0700); err != nil {
			return err
		}
		file, err := destination.OpenFile(name, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
		if err != nil {
			return err
		}
		_, writeErr := file.Write(data)
		if writeErr == nil {
			writeErr = file.Sync()
		}
		return errors.Join(writeErr, file.Close())
	})
	return inputs, errors.Join(scanErr, destination.Close())
}

func Verify(ctx context.Context, project string, resources []string, expected map[string]string) error {
	actual, err := Scan(ctx, project, resources, nil)
	if err != nil {
		return err
	}
	if !reflect.DeepEqual(actual, expected) {
		return errors.New("构建期间输入文件集合或摘要变化")
	}
	return nil
}

// Scan 使用固定根句柄、分批ReadDir和前后身份校验；不自动发现产品数据。
func Scan(ctx context.Context, project string, resources []string, consume func(string, []byte) error) (inputs map[string]string, err error) {
	names, err := resourceNames(resources)
	if err != nil {
		return nil, err
	}
	if err = ctx.Err(); err != nil {
		return nil, err
	}
	before, err := os.Lstat(project)
	if err != nil || !before.IsDir() || before.Mode()&os.ModeSymlink != 0 {
		return nil, errors.Join(errors.New("构建工程必须是实际目录"), err)
	}
	root, err := os.OpenRoot(project)
	if err != nil {
		return nil, err
	}
	defer func() { err = errors.Join(err, root.Close()) }()
	inputs = map[string]string{}
	var total int64
	var nodes int
	count := func(name string) error {
		if err := ctx.Err(); err != nil {
			return err
		}
		nodes++
		if nodes > MaxNodes || strings.Count(filepath.ToSlash(name), "/") > MaxDepth {
			return errors.New("构建输入项目数或深度超限")
		}
		return nil
	}
	copyEntry := func(name string) error {
		info, err := root.Lstat(name)
		if err != nil || !info.Mode().IsRegular() || info.Size() < 0 || info.Size() > MaxFileBytes {
			return errors.Join(errors.New("构建输入必须是有界普通文件"), err)
		}
		if total+info.Size() > MaxBytes {
			return errors.New("构建输入总量超限")
		}
		file, err := root.Open(name)
		if err != nil {
			return err
		}
		opened, statErr := file.Stat()
		if statErr != nil || !os.SameFile(info, opened) || !opened.Mode().IsRegular() || opened.Size() != info.Size() || !opened.ModTime().Equal(info.ModTime()) {
			return errors.Join(errors.New("打开期间构建输入变化"), statErr, file.Close())
		}
		data, readErr := io.ReadAll(io.LimitReader(file, MaxFileBytes+1))
		after, statErr := root.Lstat(name)
		if err = errors.Join(readErr, statErr, file.Close()); err != nil {
			return err
		}
		if !os.SameFile(info, after) || info.Mode() != after.Mode() || !info.ModTime().Equal(after.ModTime()) || int64(len(data)) != info.Size() || after.Size() != info.Size() {
			return errors.New("复制期间构建输入变化")
		}
		if consume != nil {
			if err = consume(name, data); err != nil {
				return err
			}
		}
		hash := sha256.Sum256(data)
		inputs[filepath.ToSlash(name)] = hex.EncodeToString(hash[:])
		total += int64(len(data))
		return ctx.Err()
	}
	var walk func(string, bool) error
	walk = func(name string, topLevel bool) (err error) {
		info, err := root.Lstat(name)
		if err != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			return errors.Join(errors.New("构建输入树不能含链接或非目录"), err)
		}
		directory, err := root.Open(name)
		if err != nil {
			return err
		}
		defer func() { err = errors.Join(err, directory.Close()) }()
		opened, err := directory.Stat()
		if err != nil || !os.SameFile(info, opened) {
			return errors.Join(errors.New("构建输入目录身份变化"), err)
		}
		for {
			entries, readErr := directory.ReadDir(128)
			if readErr != nil && readErr != io.EOF {
				return readErr
			}
			for _, entry := range entries {
				child := filepath.Join(name, entry.Name())
				if err = count(child); err != nil {
					return err
				}
				if topLevel && (!strings.HasSuffix(entry.Name(), ".go") || strings.HasSuffix(entry.Name(), "_test.go")) {
					continue
				}
				if entry.Type()&os.ModeSymlink != 0 {
					return errors.New("构建输入不能含链接")
				}
				if entry.IsDir() {
					if err = walk(child, false); err != nil {
						return err
					}
				} else if !strings.HasSuffix(entry.Name(), "_test.go") {
					if err = copyEntry(child); err != nil {
						return err
					}
				}
			}
			if readErr == io.EOF {
				break
			}
		}
		after, err := root.Lstat(name)
		if err != nil || !os.SameFile(info, after) || !info.ModTime().Equal(after.ModTime()) {
			return errors.Join(errors.New("扫描期间构建输入目录变化"), err)
		}
		return nil
	}
	for _, name := range names {
		if err = count(name); err != nil {
			return nil, err
		}
		// os.Root限制逃逸，父目录检查同时拒绝树内链接。
		for parent := filepath.Dir(name); parent != "."; parent = filepath.Dir(parent) {
			info, statErr := root.Lstat(parent)
			if statErr != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
				return nil, errors.Join(errors.New("构建输入父目录必须是实际目录"), statErr)
			}
		}
		if err = copyEntry(name); err != nil {
			return nil, err
		}
	}
	for _, directory := range []string{".", "internal", "frontend/dist"} {
		if directory == "frontend/dist" {
			info, statErr := root.Lstat("frontend")
			if statErr != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
				return nil, errors.Join(errors.New("前端输入父目录必须是实际目录"), statErr)
			}
		}
		if err = walk(directory, directory == "."); err != nil {
			return nil, err
		}
	}
	after, err := os.Lstat(project)
	if err != nil || !os.SameFile(before, after) {
		return nil, errors.Join(errors.New("构建工程根目录身份变化"), err)
	}
	return inputs, ctx.Err()
}
