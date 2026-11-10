//go:build !windows && (!darwin || ios)

package update

import (
	"context"
	"fmt"
	"os"
)

func platformReplaceInstall(ctx context.Context, root *os.Root, source, target string, bundle bool) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if bundle {
		return fmt.Errorf("此平台不支持应用包替换")
	}
	return root.Rename(source, target)
}
