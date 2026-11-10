//go:build !windows

package update

import (
	"context"
	"fmt"
	"os"
	"time"
)

func registeredInstallationStatus(string) (bool, error) { return false, nil }
func runElevatedUpdate(context.Context, string, []string, string, time.Duration) error {
	return fmt.Errorf("此平台不支持管理员更新")
}
func (i *installer) runWindowsInstaller(context.Context, installTask) error {
	return fmt.Errorf("此平台不支持Windows安装器")
}
func (i *installer) replaceWithElevation(ctx context.Context, journal *installJournal, source *os.Root, name string, parent *os.Root, target, digest string, _ bool) error {
	return i.replace(ctx, source, name, parent, target, journal.Task.Installation.Bundle, digest)
}
func installDirectoryFailure(err error, _ *os.Root, _ string) error { return err }

func performElevatedUpdate(context.Context, string, string, elevationOperation, BuildInfo, string) error {
	return fmt.Errorf("此平台不支持管理员更新入口")
}
