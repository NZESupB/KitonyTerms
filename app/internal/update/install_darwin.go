//go:build darwin && !ios

package update

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"golang.org/x/sys/unix"
	"os"
)

func copyInstallBundle(ctx context.Context, source *os.Root, name string, target *os.Root, destination string) (err error) {
	info, err := source.Lstat(name)
	if err != nil {
		return err
	}
	if !info.IsDir() {
		return fmt.Errorf("安装应用包不是普通目录")
	}
	directory := ".bundle-copy-" + rand.Text()
	if err = target.Mkdir(directory, 0700); err != nil {
		return err
	}
	defer func() { err = errors.Join(err, target.RemoveAll(directory)) }()
	copyRoot, err := target.OpenRoot(directory)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, copyRoot.Close()) }()
	tree := newArchiveTree(ctx, copyRoot, archiveLimits{MaxExpanded, maxArchiveEntries})
	var links []bundleLink
	if err = copyBundleEntryAt(source, tree, name, "KitonyTerms.app", &links); err != nil {
		return err
	}
	if err = createBundleLinks(tree, links); err != nil {
		return err
	}
	if err = syncInstallDirectory(copyRoot, "KitonyTerms.app"); err != nil {
		return err
	}
	return target.Rename(directory+"/KitonyTerms.app", destination)
}

func platformReplaceInstall(ctx context.Context, root *os.Root, source, target string, bundle bool) (err error) {
	if err = ctx.Err(); err != nil {
		return err
	}
	if bundle {
		if _, err = root.Lstat(target); err == nil {
			dir, err := root.Open(".")
			if err != nil {
				return err
			}
			defer func() { err = errors.Join(err, dir.Close()) }()
			return unix.RenameatxNp(int(dir.Fd()), source, int(dir.Fd()), target, unix.RENAME_SWAP)
		} else if !errors.Is(err, os.ErrNotExist) {
			return err
		}
	}
	return root.Rename(source, target)
}
