package datasnapshot

import (
	"context"
	"errors"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
)

// 分批枚举，节点/深度上限由visit在打开下级目录前检查。
func walk(ctx context.Context, root *os.Root, name string, visit func(string, fs.FileInfo) error) (err error) {
	if err = ctx.Err(); err != nil {
		return err
	}
	info, err := root.Lstat(filepath.FromSlash(name))
	if err != nil {
		return err
	}
	if err = visit(name, info); err != nil || !info.IsDir() {
		return err
	}
	directory, err := root.Open(filepath.FromSlash(name))
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, directory.Close()) }()
	opened, err := directory.Stat()
	if err != nil {
		return err
	}
	if !opened.IsDir() || !os.SameFile(info, opened) {
		return errors.New("枚举目录身份变化")
	}
	for {
		if err = ctx.Err(); err != nil {
			return err
		}
		entries, readErr := directory.ReadDir(128)
		for _, entry := range entries {
			if err = walk(ctx, root, path.Join(name, entry.Name()), visit); err != nil {
				return err
			}
		}
		if readErr != nil {
			if errors.Is(readErr, io.EOF) {
				return nil
			}
			return readErr
		}
	}
}
func rootEntries(root *os.Root) (entries []fs.DirEntry, err error) {
	file, err := root.Open(".")
	if err != nil {
		return nil, err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	entries, err = file.ReadDir(4)
	if errors.Is(err, io.EOF) {
		err = nil
	}
	return entries, err
}
