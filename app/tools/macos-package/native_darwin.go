//go:build darwin

package main

import (
	"errors"
	"os"
	"os/exec"
	"syscall"

	"golang.org/x/sys/unix"
)

func configureCommand(command *exec.Cmd) {
	command.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	command.Cancel = func() error {
		err := syscall.Kill(-command.Process.Pid, syscall.SIGKILL)
		if errors.Is(err, syscall.ESRCH) {
			return os.ErrProcessDone
		}
		return err
	}
}

func mountedReadOnly(mount, parent string) (bool, bool, error) {
	info, err := os.Lstat(mount)
	if err != nil {
		return false, false, err
	}
	base, err := os.Stat(parent)
	if err != nil {
		return false, false, err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return false, false, errors.New("候选挂载点不是实际目录")
	}
	active := info.Sys().(*syscall.Stat_t).Dev != base.Sys().(*syscall.Stat_t).Dev
	var status unix.Statfs_t
	if err = unix.Statfs(mount, &status); err != nil {
		return false, false, err
	}
	return active, status.Flags&unix.MNT_RDONLY != 0, nil
}
