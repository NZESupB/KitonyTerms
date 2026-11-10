package update

import (
	"context"
	"crypto/rand"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
)

func registeredInstallation(executable string) error {
	registered, err := registeredInstallationStatus(executable)
	if err != nil {
		return err
	}
	if !registered {
		return fmt.Errorf("安装目标未登记")
	}
	return nil
}

// 安装器任务暂存旧程序用于恢复，安装器本身始终从已验证包复制。
func (i *installer) prepareInstallerPayload(ctx context.Context, task installTask) (prepared preparedPayload, err error) {
	if err = i.held(); err != nil {
		return prepared, err
	}
	if err = task.validate(); err != nil {
		return prepared, err
	}
	if !task.Installation.Installed || task.Elevate {
		return prepared, fmt.Errorf("安装器准备任务无效")
	}
	if err = registeredInstallation(task.Installation.Executable); err != nil {
		return prepared, err
	}
	file, err := openVerifiedPackage(ctx, i.root, task.Package)
	if err != nil {
		return prepared, err
	}
	if err = file.Close(); err != nil {
		return prepared, err
	}
	parent, err := openInstallParent(task.Installation, i.root)
	if err != nil {
		return prepared, err
	}
	defer func() { err = errors.Join(err, parent.Close()) }()
	prepared.Directory = ".payload-" + rand.Text()
	if err = i.root.Mkdir(prepared.Directory, 0700); err != nil {
		return prepared, err
	}
	defer func() {
		if err != nil {
			err = errors.Join(err, removeInstallEntry(i.root, prepared.Directory))
		}
	}()
	prepared.Payload = prepared.Directory + "/previous-executable.exe"
	prepared.Executable = prepared.Payload
	if err = copyInstallFile(ctx, parent, filepath.Base(task.Installation.Target), i.root, prepared.Payload); err != nil {
		return prepared, err
	}
	prepared.SHA256, err = hashInstallFile(ctx, i.root, prepared.Payload)
	if err != nil || prepared.SHA256 != strings.ToLower(task.SourceSHA256) {
		return prepared, errors.Join(err, fmt.Errorf("安装器旧程序副本摘要不匹配"))
	}
	return prepared, nil
}
