//go:build !darwin

package main

import (
	"errors"
	"os/exec"
)

func configureCommand(command *exec.Cmd) {}
func mountedReadOnly(string, string) (bool, bool, error) {
	return false, false, errors.New("DMG原生验证需要macOS")
}
