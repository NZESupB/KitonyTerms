package update

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

type installationAccess struct {
	Elevate bool
	Reason  string
	Err     error
}

func installationAt(executable, platform string) installation {
	install := installation{Executable: executable, Target: executable}
	if platform == "darwin" && filepath.Base(executable) == "kitonyterms" && filepath.Base(filepath.Dir(executable)) == "MacOS" && filepath.Base(filepath.Dir(filepath.Dir(executable))) == "Contents" {
		target := filepath.Dir(filepath.Dir(filepath.Dir(executable)))
		if filepath.Base(target) == "KitonyTerms.app" {
			install.Target, install.Bundle = target, true
		}
	}
	return install
}

func discoverInstallation(executable string) (installation, error) {
	return inspectInstallation(executable, runtime.GOOS, registeredInstallationStatus)
}

func inspectInstallation(executable, platform string, registered func(string) (bool, error)) (installation, error) {
	if !cleanInstallPath(executable) {
		return installation{}, fmt.Errorf("当前应用路径无效")
	}
	canonical, err := canonicalUpdatePath(executable)
	if err != nil {
		return installation{}, err
	}
	info, err := os.Lstat(canonical)
	if err != nil {
		return installation{}, err
	}
	if !info.Mode().IsRegular() {
		return installation{}, fmt.Errorf("当前应用不是普通程序文件")
	}
	install := installationAt(canonical, platform)
	if platform != "windows" {
		return install, nil
	}
	parent, err := os.OpenRoot(filepath.Dir(canonical))
	if err != nil {
		return installation{}, err
	}
	var marker, uninstaller bool
	for _, row := range []struct {
		name    string
		present *bool
	}{{"kitonyterms-installation.txt", &marker}, {"unins000.exe", &uninstaller}} {
		info, err := parent.Lstat(row.name)
		if errors.Is(err, fs.ErrNotExist) {
			continue
		}
		if err != nil || !info.Mode().IsRegular() {
			return installation{}, errors.Join(err, parent.Close(), fmt.Errorf("安装登记文件必须为普通文件"))
		}
		*row.present = true
	}
	if err = parent.Close(); err != nil {
		return installation{}, err
	}
	if marker && uninstaller {
		install.Installed, err = registered(canonical)
		if err != nil {
			return installation{}, err
		}
	}
	return install, nil
}

func assessInstallation(ctx context.Context, updates *os.Root, install installation, platform string) (access installationAccess) {
	unavailable := func(reason string, err error) installationAccess {
		return installationAccess{Reason: reason, Err: err}
	}
	if err := ctx.Err(); err != nil {
		return unavailable("installation_unavailable", err)
	}
	if platform == "darwin" {
		if !install.Bundle {
			return unavailable("not_app_bundle", fmt.Errorf("macOS更新需要已安装的KitonyTerms.app"))
		}
		if install.Target == "/Volumes" || strings.HasPrefix(install.Target, "/Volumes/") {
			return unavailable("mounted_installation", fmt.Errorf("请先将应用从镜像安装到可写目录"))
		}
		if strings.Contains(install.Target, "/AppTranslocation/") {
			return unavailable("translocated_installation", fmt.Errorf("请先将应用安装到固定可写目录"))
		}
	}
	parent, err := openInstallParent(install, updates)
	if err != nil {
		return unavailable("installation_unavailable", err)
	}
	defer func() {
		if err := parent.Close(); err != nil {
			access.Elevate = false
			access.Reason = "installation_unavailable"
			access.Err = errors.Join(access.Err, err)
		}
	}()
	if install.Installed {
		return installationAccess{}
	}
	if platform == "windows" {
		info, err := parent.Lstat(filepath.Base(install.Target))
		if err != nil {
			return unavailable("installation_unavailable", err)
		}
		if info.Mode().Perm()&0222 == 0 {
			return unavailable("readonly_installation", fmt.Errorf("应用文件具有只读属性"))
		}
	}
	created, err := probeInstallationDirectory(ctx, parent)
	if err == nil {
		return installationAccess{}
	}
	if platform == "windows" && !created && errors.Is(err, fs.ErrPermission) {
		return installationAccess{Elevate: true}
	}
	return unavailable("installation_unwritable", fmt.Errorf("安装目录探测失败：%w", err))
}

// created 区分创建时的目录权限与创建后的刷盘、关闭或清理失败。
func probeInstallationDirectory(ctx context.Context, parent *os.Root) (created bool, err error) {
	if err := ctx.Err(); err != nil {
		return false, err
	}
	name := ".kitonyterms-update-probe-" + rand.Text()
	file, err := parent.OpenFile(name, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return false, err
	}
	_, writeErr := file.Write([]byte{0})
	return true, errors.Join(writeErr, file.Sync(), file.Close(), parent.Remove(name), ctx.Err())
}

// 服务必须使用进程入口已持锁的同一安装与更新目录；访问失败只禁用更新。
func (d *Desktop) inspectForService(ctx context.Context, root *os.Root, current BuildInfo) (installation, installationAccess, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.closed || d.preparation != nil || d.recovery != nil || d.current != current || !sameInstallPath(d.root.Name(), root.Name()) {
		return installation{}, installationAccess{}, fmt.Errorf("更新服务与桌面启动所有者不匹配")
	}
	if current.Channel == "development" || current.Target == "" {
		return d.install, installationAccess{}, nil
	}
	install, err := discoverInstallation(d.install.Executable)
	if err != nil {
		return installation{}, installationAccess{Reason: "installation_unavailable", Err: err}, nil
	}
	access := assessInstallation(ctx, root, install, runtime.GOOS)
	if err = ctx.Err(); err != nil {
		return installation{}, installationAccess{}, err
	}
	if access.Reason == "" {
		d.install = install
	}
	return install, access, nil
}
