package update

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
)

func hashInstallFile(ctx context.Context, root *os.Root, name string) (digest string, err error) {
	info, err := root.Lstat(name)
	if err != nil {
		return "", err
	}
	if !info.Mode().IsRegular() || info.Size() < 0 || uint64(info.Size()) > MaxExpanded {
		return "", fmt.Errorf("安装程序类型或大小无效")
	}
	file, err := root.Open(name)
	if err != nil {
		return "", err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	opened, err := file.Stat()
	if err != nil {
		return "", err
	}
	if !os.SameFile(info, opened) || !opened.Mode().IsRegular() {
		return "", fmt.Errorf("安装程序在打开时变化")
	}
	hash := sha256.New()
	n, err := io.Copy(hash, io.LimitReader(contextReader{ctx, file}, int64(MaxExpanded)+1))
	if err != nil {
		return "", err
	}
	after, err := file.Stat()
	if err != nil {
		return "", err
	}
	if n != opened.Size() || after.Size() != opened.Size() || !after.ModTime().Equal(opened.ModTime()) {
		return "", fmt.Errorf("安装程序在读取时变化")
	}
	return hex.EncodeToString(hash.Sum(nil)), nil
}

// 目标只允许新建；写句柄保持到内容、权限都刷盘完成。
func copyInstallFile(ctx context.Context, source *os.Root, name string, target *os.Root, destination string) (err error) {
	info, err := source.Lstat(name)
	if err != nil {
		return err
	}
	if !info.Mode().IsRegular() || info.Size() < 0 || uint64(info.Size()) > MaxExpanded {
		return fmt.Errorf("复制源不是有界普通文件")
	}
	input, err := source.Open(name)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, input.Close()) }()
	opened, err := input.Stat()
	if err != nil {
		return err
	}
	if !os.SameFile(info, opened) || !opened.Mode().IsRegular() {
		return fmt.Errorf("复制源在打开时变化")
	}
	output, err := target.OpenFile(destination, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, output.Close()) }()
	n, err := io.Copy(output, io.LimitReader(contextReader{ctx, input}, int64(MaxExpanded)+1))
	if err != nil {
		return err
	}
	if n != opened.Size() {
		return fmt.Errorf("复制源大小在读取时变化")
	}
	if err = output.Sync(); err != nil {
		return err
	}
	if err = output.Chmod(opened.Mode().Perm()); err != nil {
		return err
	}
	return output.Sync()
}

func copyInstallPayload(ctx context.Context, source *os.Root, name string, target *os.Root, destination string, bundle bool) error {
	if bundle {
		return copyInstallBundle(ctx, source, name, target, destination)
	}
	return copyInstallFile(ctx, source, name, target, destination)
}

func installExecutable(name string, bundle bool) string {
	if bundle {
		return filepath.Join(name, "Contents", "MacOS", "kitonyterms")
	}
	return name
}

func openInstallParent(install installation, updates *os.Root) (*os.Root, error) {
	if !cleanInstallPath(install.Target) {
		return nil, fmt.Errorf("安装目标路径无效")
	}
	parent, err := canonicalUpdatePath(filepath.Dir(install.Target))
	if err != nil {
		return nil, err
	}
	root, err := os.OpenRoot(parent)
	if err != nil {
		return nil, err
	}
	fail := func(err error) (*os.Root, error) { return nil, errors.Join(err, root.Close()) }
	// 更新目录不能包含安装目标，应用包也不能包含更新事务，避免恢复清理互相删除。
	for _, pair := range [][2]string{{filepath.Join(parent, filepath.Base(install.Target)), updates.Name()}, {updates.Name(), filepath.Join(parent, filepath.Base(install.Target))}} {
		rel, err := filepath.Rel(pair[0], pair[1])
		if err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) {
			return fail(fmt.Errorf("更新目录与安装目标重叠"))
		}
	}
	info, err := root.Lstat(filepath.Base(install.Target))
	if err == nil && (info.Mode()&fs.ModeSymlink != 0 || install.Bundle && !info.IsDir() || !install.Bundle && !info.Mode().IsRegular()) {
		return fail(fmt.Errorf("安装目标类型无效"))
	}
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fail(err)
	}
	return root, nil
}

func removeInstallEntry(root *os.Root, name string) error {
	info, err := root.Lstat(name)
	if errors.Is(err, fs.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if info.Mode()&fs.ModeSymlink != 0 {
		return fmt.Errorf("拒绝回收被替换为链接的事务路径")
	}
	return platformRemoveInstallEntry(root, name, info)
}

func chmodInstallFile(root *os.Root, name string, mode fs.FileMode) (err error) {
	file, err := root.OpenFile(name, os.O_WRONLY, 0)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	if err = file.Chmod(mode.Perm()); err != nil {
		return err
	}
	return file.Sync()
}
