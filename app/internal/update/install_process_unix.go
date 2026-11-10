//go:build unix

package update

import (
	"errors"
	"fmt"
	"golang.org/x/sys/unix"
)

func updateProcessAlive(pid uint32) (bool, error) {
	if pid == 0 || pid > 0x7fffffff {
		return false, fmt.Errorf("更新进程ID无效")
	}
	err := unix.Kill(int(pid), 0)
	if errors.Is(err, unix.ESRCH) {
		return false, nil
	}
	return err == nil, err
}
