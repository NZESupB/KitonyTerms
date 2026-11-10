//go:build windows

package update

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

// amd64 的 SHELLEXECUTEINFOW；当前 Windows 发布目标仅 amd64。
type updateShellExecuteInfo struct {
	Size, Mask                        uint32
	Window                            windows.Handle
	Verb, File, Parameters, Directory *uint16
	Show                              int32
	Instance                          windows.Handle
	IDList                            unsafe.Pointer
	Class                             *uint16
	ClassKey                          windows.Handle
	HotKey                            uint32
	Icon                              windows.Handle
	Process                           windows.Handle
}

func runElevatedUpdate(ctx context.Context, file string, arguments []string, expected string, budget time.Duration) (err error) {
	if err := ctx.Err(); err != nil {
		return err
	}
	if !validHex(expected, 64) {
		return fmt.Errorf("管理员启动缺少已验证摘要")
	}
	pins, err := pinUpdateDirectories(filepath.Dir(file))
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, closeUpdatePins(pins)) }()
	input, err := pinUpdatePath(file, false)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, input.Close()) }()
	parent, err := os.OpenRoot(filepath.Dir(file))
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	digest, err := hashInstallFile(ctx, parent, filepath.Base(file))
	if err != nil || digest != strings.ToLower(expected) {
		return errors.Join(err, fmt.Errorf("管理员启动程序摘要不匹配"))
	}
	compatible, err := windowsExternalPath(file)
	if err != nil {
		return err
	}
	params, err := windowsCommandArguments(arguments)
	if err != nil {
		return err
	}
	fileUnits, err := windows.UTF16PtrFromString(compatible)
	if err != nil {
		return err
	}
	parameterUnits, err := windows.UTF16PtrFromString(params)
	if err != nil {
		return err
	}
	verb, _ := windows.UTF16PtrFromString("runas")
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	// COM 必须在与 ShellExecuteExW 相同的线程初始化和释放。
	if err = windows.CoInitializeEx(0, windows.COINIT_APARTMENTTHREADED|windows.COINIT_DISABLE_OLE1DDE); err != nil && err != syscall.Errno(1) {
		return fmt.Errorf("初始化管理员启动COM失败：%w", err)
	}
	defer windows.CoUninitialize()
	info := updateShellExecuteInfo{Mask: 0x40 | 0x100, Verb: verb, File: fileUnits, Parameters: parameterUnits, Show: windows.SW_SHOWNORMAL}
	info.Size = uint32(unsafe.Sizeof(info))
	result, _, callErr := windows.NewLazySystemDLL("shell32.dll").NewProc("ShellExecuteExW").Call(uintptr(unsafe.Pointer(&info)))
	runtime.KeepAlive(fileUnits)
	runtime.KeepAlive(parameterUnits)
	if result == 0 {
		if errors.Is(callErr, windows.ERROR_CANCELLED) {
			return fmt.Errorf("已取消管理员授权，保留当前版本：%w", callErr)
		}
		return fmt.Errorf("无法启动管理员程序：%w", callErr)
	}
	if info.Process == 0 {
		return fmt.Errorf("管理员启动缺少进程句柄")
	}
	defer windows.CloseHandle(info.Process)
	return waitElevatedExit(ctx, budget, func() (bool, uint32, error) {
		wait, waitErr := windows.WaitForSingleObject(info.Process, 100)
		var code uint32
		readErr := windows.GetExitCodeProcess(info.Process, &code)
		if wait == windows.WAIT_OBJECT_0 {
			return true, code, errors.Join(waitErr, readErr)
		}
		if waitErr != nil && readErr == nil && code != 259 {
			return true, code, waitErr
		}
		return false, code, errors.Join(waitErr, readErr)
	})
}

func pinUpdatePath(path string, directory bool) (*os.File, error) {
	return pinUpdateHandle(path, directory, false)
}

func pinUpdateHandle(path string, directory, output bool) (*os.File, error) {
	units, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return nil, err
	}
	// 元数据访问不会参与读写删除共享检查；日志和目录也必须实际请求读取。
	access, share := uint32(windows.GENERIC_READ), uint32(windows.FILE_SHARE_READ)
	flags := uint32(windows.FILE_FLAG_OPEN_REPARSE_POINT)
	if directory {
		share |= windows.FILE_SHARE_WRITE
		flags |= windows.FILE_FLAG_BACKUP_SEMANTICS
	}
	if output {
		share |= windows.FILE_SHARE_WRITE
	}
	handle, err := windows.CreateFile(units, access, share, nil, windows.OPEN_EXISTING, flags, 0)
	if err != nil {
		return nil, fmt.Errorf("固定管理员事务路径失败：%w", err)
	}
	file := os.NewFile(uintptr(handle), path)
	var info windows.ByHandleFileInformation
	if err = windows.GetFileInformationByHandle(handle, &info); err == nil {
		if info.FileAttributes&windows.FILE_ATTRIBUTE_REPARSE_POINT != 0 || (info.FileAttributes&windows.FILE_ATTRIBUTE_DIRECTORY != 0) != directory {
			err = fmt.Errorf("管理员路径包含重解析点或类型变化")
		}
		if !directory && info.NumberOfLinks != 1 {
			err = fmt.Errorf("管理员输入不允许硬链接")
		}
	}
	if err != nil {
		return nil, errors.Join(err, file.Close())
	}
	return file, nil
}

// 自卷根向下固定每一级目录，禁止在校验与替换之间把父目录换成 junction。
func pinUpdateDirectories(paths ...string) (files []*os.File, err error) {
	seen := make(map[string]bool)
	defer func() {
		if err != nil {
			err = errors.Join(err, closeUpdatePins(files))
			files = nil
		}
	}()
	for _, path := range paths {
		if !filepath.IsAbs(path) {
			return files, fmt.Errorf("管理员事务目录必须绝对")
		}
		var ancestors []string
		for current := filepath.Clean(path); ; current = filepath.Dir(current) {
			ancestors = append(ancestors, current)
			if filepath.Dir(current) == current {
				break
			}
		}
		for index := len(ancestors) - 1; index >= 0; index-- {
			ancestor := ancestors[index]
			if seen[ancestor] {
				continue
			}
			file, openErr := pinUpdatePath(ancestor, true)
			if openErr != nil {
				return files, openErr
			}
			files = append(files, file)
			seen[ancestor] = true
		}
	}
	return files, nil
}

func closeUpdatePins(files []*os.File) error {
	var err error
	for index := len(files) - 1; index >= 0; index-- {
		err = errors.Join(err, files[index].Close())
	}
	return err
}
