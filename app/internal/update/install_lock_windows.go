//go:build windows

package update

import (
	"errors"
	"golang.org/x/sys/windows"
	"os"
)

func tryInstallFileLock(file *os.File) (bool, error) {
	err := windows.LockFileEx(windows.Handle(file.Fd()), windows.LOCKFILE_EXCLUSIVE_LOCK|windows.LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &windows.Overlapped{})
	if errors.Is(err, windows.ERROR_LOCK_VIOLATION) {
		return false, nil
	}
	return err == nil, err
}

// Windows 没有目录 fsync；数据文件均在写句柄关闭前 FlushFileBuffers。
func syncDirectoryFile(_ *os.File) error { return nil }
