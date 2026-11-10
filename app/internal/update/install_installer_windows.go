//go:build windows

package update

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"golang.org/x/sys/windows/registry"
)

func registeredInstallationStatus(executable string) (bool, error) {
	if !strings.EqualFold(filepath.Base(executable), "kitonyterms.exe") {
		return false, nil
	}
	expected, err := canonicalUpdatePath(filepath.Dir(executable))
	if err != nil {
		return false, err
	}
	const keyPath = `SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\KitonyTerms_is1`
	for _, view := range []uint32{registry.WOW64_32KEY, registry.WOW64_64KEY} {
		key, openErr := registry.OpenKey(registry.LOCAL_MACHINE, keyPath, registry.QUERY_VALUE|view)
		if errors.Is(openErr, registry.ErrNotExist) {
			continue
		}
		if openErr != nil {
			return false, fmt.Errorf("读取安装登记失败：%w", openErr)
		}
		location, kind, readErr := key.GetStringValue("InstallLocation")
		closeErr := key.Close()
		if readErr != nil && !errors.Is(readErr, registry.ErrNotExist) {
			return false, fmt.Errorf("读取安装目录失败：%w", errors.Join(readErr, closeErr))
		}
		if closeErr != nil {
			return false, closeErr
		}
		if readErr != nil || kind != registry.SZ || !cleanInstallPath(filepath.Clean(location)) {
			continue
		}
		canonical, resolveErr := canonicalUpdatePath(location)
		if resolveErr != nil {
			if errors.Is(resolveErr, os.ErrNotExist) {
				continue
			}
			return false, fmt.Errorf("解析安装登记目录失败：%w", resolveErr)
		}
		if strings.EqualFold(canonical, expected) {
			return true, nil
		}
	}
	return false, nil
}

func (i *installer) runWindowsInstaller(ctx context.Context, task installTask) (err error) {
	if err = i.held(); err != nil {
		return err
	}
	if !task.Installation.Installed || task.Elevate || task.Package.Kind != "installer" {
		return fmt.Errorf("系统安装器任务无效")
	}
	if err = registeredInstallation(task.Installation.Executable); err != nil {
		return err
	}
	verified, err := openVerifiedPackage(ctx, i.root, task.Package)
	if err != nil {
		return err
	}
	if err = verified.Close(); err != nil {
		return err
	}
	for _, name := range []string{"update-setup.exe", "update-install.log"} {
		if err = removeInstallEntry(i.root, name); err != nil {
			return err
		}
	}
	if err = copyInstallFile(ctx, i.root, "package-"+task.Package.SHA256, i.root, "update-setup.exe"); err != nil {
		return err
	}
	pins, err := pinUpdateDirectories(i.root.Name(), filepath.Dir(task.Installation.Target))
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, closeUpdatePins(pins)) }()
	setup, err := pinUpdatePath(filepath.Join(i.root.Name(), "update-setup.exe"), false)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, setup.Close()) }()
	digest, err := hashInstallFile(ctx, i.root, "update-setup.exe")
	if err != nil || digest != strings.ToLower(task.Package.SHA256) {
		return errors.Join(err, fmt.Errorf("系统安装器副本摘要不匹配"))
	}
	logFile, err := i.root.OpenFile("update-install.log", os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	if err = errors.Join(logFile.Sync(), logFile.Close()); err != nil {
		return err
	}
	logPin, err := pinUpdateHandle(filepath.Join(i.root.Name(), "update-install.log"), false, true)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, logPin.Close()) }()
	args, err := windowsInstallerArguments(filepath.Dir(task.Installation.Executable), filepath.Join(i.root.Name(), "update-install.log"))
	if err != nil {
		return err
	}
	if err = verifyRestoredInstall(ctx, i.root, task); err != nil {
		return err
	}
	if err = runElevatedUpdate(ctx, filepath.Join(i.root.Name(), "update-setup.exe"), args, task.Package.SHA256, 10*time.Minute); err != nil {
		return fmt.Errorf("系统安装器执行阶段失败，日志位于updates/update-install.log：%w", err)
	}
	return nil
}

func (i *installer) replaceWithElevation(ctx context.Context, journal *installJournal, source *os.Root, name string, parent *os.Root, target, digest string, restore bool) error {
	if journal.Task.Elevate || restore && journal.Task.Installation.Installed {
		operation := elevationReplace
		if restore {
			operation = elevationRestore
		}
		return i.requestElevation(ctx, operation)
	}
	err := i.replace(ctx, source, name, parent, target, false, digest)
	if err == nil || !replacementNeedsElevation(err, journal.Task.Installation.Target) {
		return err
	}
	if !restore {
		if verifyErr := verifyRestoredInstall(ctx, i.root, journal.Task); verifyErr != nil {
			return errors.Join(err, verifyErr)
		}
		journal.Task.Elevate = true
		if err = i.writeJournal(*journal); err != nil {
			return err
		}
		if err = writeInstallRecord(i.root, "elevation.json", true); err != nil {
			return err
		}
		return i.requestElevation(ctx, elevationReplace)
	}
	return i.requestElevation(ctx, elevationRestore)
}
