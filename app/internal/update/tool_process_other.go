//go:build !windows

package update

import (
	"context"
	"io"
	"os/exec"
	"time"
)

func executeUpdateTool(ctx context.Context, executable string, arguments []string, stdout, stderr io.Writer) error {
	command := exec.CommandContext(ctx, executable, arguments...)
	command.Stdout, command.Stderr = stdout, stderr
	command.WaitDelay = 2 * time.Second
	return command.Run()
}
