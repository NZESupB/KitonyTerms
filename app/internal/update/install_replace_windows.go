//go:build windows

package update

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"golang.org/x/sys/windows"
)

func platformReplaceInstall(ctx context.Context, root *os.Root, source, target string, _ bool) error {
	deadline := time.Now().Add(3 * time.Second)
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		if err := rejectWindowsInstallReparse(filepath.Join(root.Name(), target)); err != nil {
			return err
		}
		err := root.Rename(source, target)
		if err == nil {
			return nil
		}
		// 映像退出后的短暂拒绝可能先于写探测消失；先用同一预算重试，再判断是否提权。
		failure := installDirectoryFailure(err, root, target)
		kind := failure.Kind
		if kind != "in_use" && kind != "access_denied" || !time.Now().Before(deadline) {
			return failure
		}
		timer := time.NewTimer(min(100*time.Millisecond, time.Until(deadline)))
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-timer.C:
		}
	}
}

// 映射中的 EXE 有时返回5；无截断写探测只重新分类，不绕过占用。
func installDirectoryFailure(err error, root *os.Root, target string) *replaceFailure {
	kind := replaceErrorKind(err)
	if kind == "access_denied" {
		file, probe := root.OpenFile(target, os.O_WRONLY, 0)
		if probe == nil {
			_ = file.Close()
		} else if replaceErrorKind(probe) == "in_use" {
			kind = "in_use"
		}
	}
	return &replaceFailure{Target: filepath.Join(root.Name(), target), Kind: kind, Err: err}
}

func rejectWindowsInstallReparse(path string) error {
	units, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return err
	}
	attributes, err := windows.GetFileAttributes(units)
	if errors.Is(err, windows.ERROR_FILE_NOT_FOUND) {
		return nil
	}
	if err != nil {
		return err
	}
	if attributes&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 {
		return fmt.Errorf("安装目标不允许重解析点")
	}
	return nil
}

// 映像进程已退出后仍可能短暂占用文件；只等待明确的拒绝/占用并持续绑定原路径身份。
func platformRemoveInstallEntry(root *os.Root, name string, expected os.FileInfo) error {
	deadline := time.Now().Add(3 * time.Second)
	for {
		actual, err := root.Lstat(name)
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		if err != nil {
			return err
		}
		if actual.Mode()&os.ModeSymlink != 0 || !os.SameFile(expected, actual) {
			return fmt.Errorf("回收事务路径在等待时被替换，保留现有材料")
		}
		err = root.RemoveAll(name)
		if err == nil {
			return nil
		}
		kind := replaceErrorKind(err)
		if kind != "in_use" && kind != "access_denied" || !time.Now().Before(deadline) {
			return err
		}
		time.Sleep(min(100*time.Millisecond, time.Until(deadline)))
	}
}
