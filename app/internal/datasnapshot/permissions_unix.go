//go:build !windows

package datasnapshot

import "os"

// Unix目标由Mkdir(0700)及OpenFile(0600/0700)创建。
func protectDestination(_ *os.Root) error { return nil }
