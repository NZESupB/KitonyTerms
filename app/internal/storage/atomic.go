package storage

import (
	"fmt"
	"os"
	"path/filepath"
)

// AtomicWrite 先刷盘再提交；失败保留旧文件，临时文件仅位于目标目录。
func AtomicWrite(path string, data []byte) error {
	if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), ".kitony-write-*")
	if err != nil {
		return err
	}
	temporary := f.Name()
	defer os.Remove(temporary)
	if err = f.Chmod(0600); err == nil {
		_, err = f.Write(data)
	}
	if err == nil {
		err = f.Sync()
	}
	closeErr := f.Close()
	if err != nil {
		return err
	}
	if closeErr != nil {
		return closeErr
	}
	if err = replaceFile(temporary, path); err != nil {
		return fmt.Errorf("提交 %s: %w", filepath.Base(path), err)
	}
	return nil
}

// CommitFile 提交由同目录临时文件完成的流式下载。
func CommitFile(source, target string) error {
	if filepath.Clean(filepath.Dir(source)) != filepath.Clean(filepath.Dir(target)) {
		return fmt.Errorf("临时文件必须位于目标目录")
	}
	return replaceFile(source, target)
}
