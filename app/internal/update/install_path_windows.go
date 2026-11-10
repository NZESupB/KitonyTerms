//go:build windows

package update

import (
	"fmt"
	"golang.org/x/sys/windows"
)

func canonicalUpdatePath(path string) (string, error) {
	units, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return "", err
	}
	handle, err := windows.CreateFile(units, 0, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE, nil, windows.OPEN_EXISTING, windows.FILE_FLAG_BACKUP_SEMANTICS, 0)
	if err != nil {
		return "", err
	}
	defer windows.CloseHandle(handle)
	buffer := make([]uint16, 512)
	for {
		size, err := windows.GetFinalPathNameByHandle(handle, &buffer[0], uint32(len(buffer)), 0)
		if err != nil {
			return "", err
		}
		if size < uint32(len(buffer)) {
			return windows.UTF16ToString(buffer[:size]), nil
		}
		if size >= 32767 {
			return "", fmt.Errorf("更新规范路径过长")
		}
		buffer = make([]uint16, size+1)
	}
}
