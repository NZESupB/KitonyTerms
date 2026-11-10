//go:build darwin && !ios

package update

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"strings"
	"syscall"
	"time"
	"unicode/utf8"
)

var dmgGate = make(chan struct{}, 1)

func mountedDMG(mount, parent string) (bool, error) {
	info, err := os.Lstat(mount)
	if err != nil {
		return false, err
	}
	if !info.IsDir() || info.Mode()&fs.ModeSymlink != 0 {
		return false, fmt.Errorf("DMG 挂载点不是普通目录")
	}
	base, err := os.Stat(parent)
	if err != nil {
		return false, err
	}
	return info.Sys().(*syscall.Stat_t).Dev != base.Sys().(*syscall.Stat_t).Dev, nil
}

func detachDMG(mount string) error {
	_, err := runUpdateTool(context.Background(), 20*time.Second, 64<<10, "/usr/bin/hdiutil", "detach", mount)
	if err == nil {
		return nil
	}
	_, force := runUpdateTool(context.Background(), 20*time.Second, 64<<10, "/usr/bin/hdiutil", "detach", "-force", mount)
	if force != nil {
		return errors.Join(err, force)
	}
	return nil
}

func extractDMG(ctx context.Context, file *os.File, tree *archiveTree, pkg Package) (err error) {
	select {
	case dmgGate <- struct{}{}:
		defer func() { <-dmgGate }()
	case <-ctx.Done():
		return ctx.Err()
	}
	parent := filepath.Dir(tree.root.Name())
	mount := filepath.Join(parent, ".dmg-mount")
	if _, statErr := os.Lstat(mount); statErr == nil {
		active, checkErr := mountedDMG(mount, parent)
		if checkErr != nil {
			return checkErr
		}
		if active {
			if err = detachDMG(mount); err != nil {
				return err
			}
		}
		if err = os.Remove(mount); err != nil {
			return fmt.Errorf("旧 DMG 挂载目录非空或不可删除：%w", err)
		}
	} else if !errors.Is(statErr, fs.ErrNotExist) {
		return statErr
	}
	if err = os.Mkdir(mount, 0700); err != nil {
		return err
	}
	defer func() {
		active, checkErr := mountedDMG(mount, parent)
		if checkErr != nil {
			err = errors.Join(err, checkErr)
			return
		}
		if active {
			if cleanup := detachDMG(mount); cleanup != nil {
				err = errors.Join(err, fmt.Errorf("DMG 卸载失败，挂载点已保留：%w", cleanup))
				return
			}
		}
		err = errors.Join(err, os.Remove(mount))
	}()
	// 镜像工具按路径读取；先从已验证句柄复制并再次校验，避免重新打开原缓存。
	image, err := tree.root.OpenFile(".input.dmg", os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	hash := sha256.New()
	n, copyErr := io.Copy(io.MultiWriter(image, hash), io.LimitReader(contextReader{ctx, file}, int64(pkg.Size)+1))
	if copyErr == nil {
		copyErr = image.Sync()
	}
	copyErr = errors.Join(copyErr, image.Close())
	defer func() { err = errors.Join(err, tree.root.Remove(".input.dmg")) }()
	if copyErr != nil {
		return copyErr
	}
	if n != int64(pkg.Size) || hex.EncodeToString(hash.Sum(nil)) != strings.ToLower(pkg.SHA256) {
		return fmt.Errorf("DMG 暂存摘要不匹配")
	}
	imagePath := filepath.Join(tree.root.Name(), ".input.dmg")
	if _, err = runUpdateTool(ctx, 90*time.Second, 64<<10, "/usr/bin/hdiutil", "verify", "-quiet", imagePath); err != nil {
		return err
	}
	if _, err = runUpdateTool(ctx, 90*time.Second, 64<<10, "/usr/bin/hdiutil", "attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint", mount, imagePath); err != nil {
		return err
	}
	active, err := mountedDMG(mount, parent)
	if err != nil {
		return err
	}
	if !active {
		return fmt.Errorf("更新镜像未挂载")
	}
	source, err := os.OpenRoot(mount)
	if err != nil {
		return err
	}
	defer source.Close()
	var links []bundleLink
	if err = copyBundleEntry(source, tree, "KitonyTerms.app", &links); err != nil {
		return err
	}
	return createBundleLinks(tree, links)
}

type bundleLink struct{ name, target string }

func validateBundleLink(name, target string) error {
	if !utf8.ValidString(target) || len(target) > 4096 || strings.ContainsAny(target, "\\:") || strings.HasPrefix(target, "/") || target == "" {
		return fmt.Errorf("应用链接目标无效")
	}
	resolved := strings.Split(path.Dir(name), "/")
	if len(resolved) == 0 || resolved[0] != "KitonyTerms.app" {
		return fmt.Errorf("应用链接必须位于包内")
	}
	for _, part := range strings.Split(target, "/") {
		switch part {
		case ".":
		case "..":
			if len(resolved) <= 1 {
				return fmt.Errorf("应用链接越界")
			}
			resolved = resolved[:len(resolved)-1]
		default:
			if !archiveComponent(part) {
				return fmt.Errorf("应用链接路径无效")
			}
			resolved = append(resolved, part)
		}
	}
	return nil
}

func copyBundleEntry(source *os.Root, tree *archiveTree, raw string, links *[]bundleLink) error {
	return copyBundleEntryAt(source, tree, raw, raw, links)
}

func copyBundleEntryAt(source *os.Root, tree *archiveTree, sourceName, raw string, links *[]bundleLink) error {
	info, err := source.Lstat(sourceName)
	if err != nil {
		return err
	}
	name, err := tree.claim(raw, info.IsDir())
	if err != nil {
		return err
	}
	switch {
	case info.Mode()&fs.ModeSymlink != 0:
		target, err := source.Readlink(sourceName)
		if err != nil {
			return err
		}
		if err = validateBundleLink(name, target); err != nil {
			return err
		}
		*links = append(*links, bundleLink{name, target})
		return nil
	case info.IsDir():
		if err = tree.directory(name); err != nil {
			return err
		}
		dir, err := source.Open(sourceName)
		if err != nil {
			return err
		}
		defer dir.Close()
		opened, err := dir.Stat()
		if err != nil {
			return err
		}
		if !os.SameFile(info, opened) || !opened.IsDir() {
			return fmt.Errorf("DMG 目录在打开时改变")
		}
		for {
			entries, readErr := dir.ReadDir(128)
			for _, entry := range entries {
				if err = copyBundleEntryAt(source, tree, path.Join(sourceName, entry.Name()), path.Join(name, entry.Name()), links); err != nil {
					return err
				}
			}
			if readErr == io.EOF {
				return syncInstallDirectory(tree.root, name)
			}
			if readErr != nil {
				return readErr
			}
		}
	case info.Mode().IsRegular():
		file, err := source.Open(sourceName)
		if err != nil {
			return err
		}
		opened, err := file.Stat()
		if err == nil && (!os.SameFile(info, opened) || !opened.Mode().IsRegular()) {
			err = fmt.Errorf("DMG 文件在打开时改变")
		}
		if err == nil {
			err = tree.write(name, uint64(info.Size()), info.Mode(), file)
		}
		return errors.Join(err, file.Close())
	default:
		return fmt.Errorf("DMG 包含特殊文件")
	}
}

func createBundleLinks(tree *archiveTree, links []bundleLink) error {
	for _, link := range links {
		if err := tree.ctx.Err(); err != nil {
			return err
		}
		if err := tree.root.Symlink(link.target, link.name); err != nil {
			return err
		}
		if err := syncInstallDirectory(tree.root, path.Dir(link.name)); err != nil {
			return err
		}
	}
	bundle, err := filepath.EvalSymlinks(filepath.Join(tree.root.Name(), "KitonyTerms.app"))
	if err != nil {
		return err
	}
	for _, link := range links {
		resolved, err := filepath.EvalSymlinks(filepath.Join(tree.root.Name(), filepath.FromSlash(link.name)))
		if err != nil {
			return err
		}
		relative, err := filepath.Rel(bundle, resolved)
		if err != nil || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
			return fmt.Errorf("应用链接解析后越界")
		}
	}
	return tree.ctx.Err()
}

func verifyBundle(ctx context.Context, bundle string) error {
	if _, err := runUpdateTool(ctx, 30*time.Second, 64<<10, "/usr/bin/codesign", "--verify", "--deep", "--strict", "--verbose=4", bundle); err != nil {
		return fmt.Errorf("更新应用签名校验失败：%w", err)
	}
	for key, expected := range map[string]string{"CFBundleIdentifier": "com.kitonyterms.app", "CFBundleExecutable": "kitonyterms"} {
		value, err := runUpdateTool(ctx, 10*time.Second, 4096, "/usr/libexec/PlistBuddy", "-c", "Print :"+key, filepath.Join(bundle, "Contents/Info.plist"))
		if err != nil {
			return err
		}
		if strings.TrimSpace(string(value)) != expected {
			return fmt.Errorf("更新应用包身份不匹配")
		}
	}
	return nil
}
