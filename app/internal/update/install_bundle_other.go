//go:build !darwin || ios

package update

import (
	"context"
	"fmt"
	"os"
)

func copyInstallBundle(_ context.Context, _ *os.Root, _ string, _ *os.Root, _ string) error {
	return fmt.Errorf("此平台不支持应用包安装")
}
