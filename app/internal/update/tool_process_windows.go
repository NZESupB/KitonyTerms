//go:build windows

package update

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"time"
)

// 直接传入CreateProcess的应用路径，避免os/exec把无扩展名载荷改选为邻近的.exe。
func executeUpdateTool(ctx context.Context, executable string, arguments []string, stdout, stderr io.Writer) error {
	if !filepath.IsAbs(executable) {
		return fmt.Errorf("Windows更新工具必须使用显式绝对路径")
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	input, err := os.Open(os.DevNull)
	if err != nil {
		return err
	}
	defer input.Close()
	outRead, outWrite, err := os.Pipe()
	if err != nil {
		return err
	}
	defer outRead.Close()
	defer outWrite.Close()
	errRead, errWrite, err := os.Pipe()
	if err != nil {
		return err
	}
	defer errRead.Close()
	defer errWrite.Close()
	process, err := os.StartProcess(executable, append([]string{executable}, arguments...), &os.ProcAttr{
		Files: []*os.File{input, outWrite, errWrite},
	})
	if err != nil {
		return err
	}
	closeErr := errors.Join(outWrite.Close(), errWrite.Close())
	killed := make(chan struct{})
	stopKill := context.AfterFunc(ctx, func() {
		_ = process.Kill()
		close(killed)
	})
	defer func() {
		if !stopKill() {
			<-killed
		}
	}()
	copied := make(chan error, 2)
	go func() { _, err := io.Copy(stdout, outRead); copied <- err }()
	go func() { _, err := io.Copy(stderr, errRead); copied <- err }()
	state, waitErr := process.Wait()
	if waitErr == nil && !state.Success() {
		waitErr = &exec.ExitError{ProcessState: state}
	}
	result := errors.Join(closeErr, waitErr)
	// 与其它平台的WaitDelay一致；后代进程持有管道时也限时回收并实际等待读取结束。
	timer := time.NewTimer(2 * time.Second)
	defer timer.Stop()
	for remaining := 2; remaining > 0; remaining-- {
		select {
		case err := <-copied:
			result = errors.Join(result, err)
		case <-timer.C:
			_ = outRead.Close()
			_ = errRead.Close()
			for ; remaining > 0; remaining-- {
				result = errors.Join(result, <-copied)
			}
			return errors.Join(result, exec.ErrWaitDelay)
		}
	}
	return result
}
