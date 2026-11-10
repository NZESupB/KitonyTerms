//go:build !darwin || ios

package update

import (
	"context"
	"fmt"
	"os"
)

func extractDMG(context.Context, *os.File, *archiveTree, Package) error {
	return fmt.Errorf("此平台不能处理 DMG 更新包")
}

func verifyBundle(context.Context, string) error {
	return fmt.Errorf("此平台不能验证 macOS 应用包")
}
