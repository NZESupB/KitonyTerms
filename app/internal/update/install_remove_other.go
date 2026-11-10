//go:build !windows

package update

import "os"

func platformRemoveInstallEntry(root *os.Root, name string, _ os.FileInfo) error {
	return root.RemoveAll(name)
}
