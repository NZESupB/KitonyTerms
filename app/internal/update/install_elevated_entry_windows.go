//go:build windows

package update

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// 管理员仅完成一次已验证的文件替换，不获取协调锁、不清理事务、不启动GUI。
// 普通协调器在等待此入口退出期间持续持有 helper/installation/应用锁。
func performElevatedUpdate(ctx context.Context, rootPath, digest string, operation elevationOperation, current BuildInfo, self string) (err error) {
	if !cleanInstallPath(rootPath) || !validHex(digest, 64) || operation.argument() == "" {
		return fmt.Errorf("管理员事务参数无效")
	}
	canonical, err := canonicalUpdatePath(rootPath)
	if err != nil || canonical != rootPath {
		return errors.Join(err, fmt.Errorf("管理员事务目录不是规范路径"))
	}
	if current.Target != "x86_64-pc-windows-msvc" && current.Target != "aarch64-pc-windows-msvc" {
		return fmt.Errorf("管理员入口仅支持Windows正式目标")
	}
	rootPins, err := pinUpdateDirectories(rootPath)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, closeUpdatePins(rootPins)) }()
	root, err := os.OpenRoot(rootPath)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, root.Close()) }()
	// 自身必须先绑定固定位置，之后才允许向原用户目录写结果。
	selfPath, err := canonicalUpdatePath(self)
	if err != nil || selfPath != filepath.Join(rootPath, updateHelperName()) {
		return errors.Join(err, fmt.Errorf("管理员入口位置不匹配"))
	}
	selfPin, err := pinUpdatePath(selfPath, false)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, selfPin.Close()) }()
	result := func() (err error) {
		record, err := pinUpdatePath(filepath.Join(rootPath, "journal.json"), false)
		if err != nil {
			return err
		}
		defer func() { err = errors.Join(err, record.Close()) }()
		journal, actual, err := readInstallJournal(root)
		if err != nil {
			return err
		}
		if actual != strings.ToLower(digest) {
			return fmt.Errorf("管理员事务记录摘要已变化")
		}
		install := journal.Task.Installation
		engine := &installer{root: root}
		parent, err := openInstallParent(install, root)
		if err != nil {
			return err
		}
		defer func() { err = errors.Join(err, parent.Close()) }()
		backupRoot, backup := engine.backup(journal, parent)
		sourcePath := filepath.Join(journal.Staging, "payload")
		expected := strings.ToLower(journal.StagedSHA256)
		if operation == elevationRestore {
			sourcePath = filepath.Join(backupRoot.Name(), backup)
			expected = strings.ToLower(journal.Task.SourceSHA256)
		}
		directories, err := pinUpdateDirectories(filepath.Dir(install.Target), filepath.Dir(sourcePath))
		if err != nil {
			return err
		}
		defer func() { err = errors.Join(err, closeUpdatePins(directories)) }()
		// 固定恢复源；替换还必须固定原始包，使其验证后无法被改写。
		source, err := pinUpdatePath(sourcePath, false)
		if err != nil {
			return err
		}
		defer func() { err = errors.Join(err, source.Close()) }()
		if operation == elevationReplace {
			pkg, err := pinUpdatePath(filepath.Join(rootPath, "package-"+journal.Task.Package.SHA256), false)
			if err != nil {
				return err
			}
			defer func() { err = errors.Join(err, pkg.Close()) }()
		}
		if err = validateElevatedRequest(ctx, root, journal, current, self, operation); err != nil {
			return err
		}
		sourceRoot, err := os.OpenRoot(filepath.Dir(sourcePath))
		if err != nil {
			return err
		}
		defer func() { err = errors.Join(err, sourceRoot.Close()) }()
		actual, err = hashInstallFile(ctx, sourceRoot, filepath.Base(sourcePath))
		if err != nil || actual != expected {
			return errors.Join(err, fmt.Errorf("管理员载荷摘要不匹配"))
		}
		if operation == elevationReplace {
			if err = validateBinary(sourceRoot, filepath.Base(sourcePath), journal.Task.Next.Target); err != nil {
				return err
			}
		}
		if err = engine.replace(ctx, sourceRoot, filepath.Base(sourcePath), parent, filepath.Base(install.Target), false, expected); err != nil {
			return err
		}
		actual, err = hashInstallFile(ctx, parent, filepath.Base(install.Target))
		if err != nil || actual != expected {
			return errors.Join(err, fmt.Errorf("管理员替换结果摘要不匹配"))
		}
		return nil
	}()
	outcome := elevatedOutcome{Operation: operation, JournalSHA256: strings.ToLower(digest)}
	if result != nil {
		message := result.Error()
		outcome.Error = &message
	}
	return errors.Join(result, writeInstallRecord(root, "elevated-result.json", outcome))
}
