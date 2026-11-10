//go:build windows

package update

import (
	"errors"
	"fmt"
	"golang.org/x/sys/windows"
)

func updateProcessAlive(pid uint32) (bool, error) {
	if pid == 0 || pid > 0x7fffffff {
		return false, fmt.Errorf("更新进程ID无效")
	}
	handle, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
	if errors.Is(err, windows.ERROR_INVALID_PARAMETER) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	defer windows.CloseHandle(handle)
	var code uint32
	if err = windows.GetExitCodeProcess(handle, &code); err != nil {
		return false, err
	}
	return code == 259, nil
}
