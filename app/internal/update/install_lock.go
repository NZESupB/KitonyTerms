package update

import (
	"context"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"time"
)

// 锁文件一直保留；删除锁文件会让旧、新文件句柄分别持有两个互不相干的锁。
type installLock struct{ file *os.File }

func acquireInstallLock(ctx context.Context, root *os.Root, name string, wait time.Duration) (*installLock, error) {
	if name != "installation.lock" && name != "helper.lock" && name != "kitonyterms.lock" {
		return nil, fmt.Errorf("更新锁名称无效")
	}
	file, err := root.OpenFile(name, os.O_RDWR|os.O_CREATE, 0600)
	if err != nil {
		return nil, err
	}
	fail := func(err error) (*installLock, error) { return nil, errors.Join(err, file.Close()) }
	opened, err := file.Stat()
	if err != nil {
		return fail(err)
	}
	info, err := root.Lstat(name)
	if err != nil {
		return fail(err)
	}
	if !info.Mode().IsRegular() || !os.SameFile(info, opened) {
		return fail(fmt.Errorf("更新锁不是原普通文件"))
	}
	deadline := time.Now().Add(wait)
	for {
		if err = ctx.Err(); err != nil {
			return fail(err)
		}
		locked, lockErr := tryInstallFileLock(file)
		if lockErr != nil {
			return fail(lockErr)
		}
		if locked {
			current, err := root.Lstat(name)
			if err != nil || !os.SameFile(opened, current) {
				return fail(fmt.Errorf("更新锁在加锁时改变"))
			}
			return &installLock{file}, nil
		}
		if wait <= 0 || !time.Now().Before(deadline) {
			return fail(fmt.Errorf("更新锁正由其他进程持有"))
		}
		timer := time.NewTimer(min(50*time.Millisecond, time.Until(deadline)))
		select {
		case <-ctx.Done():
			timer.Stop()
			return fail(ctx.Err())
		case <-timer.C:
		}
	}
}

func (lock *installLock) Close() error {
	if lock == nil || lock.file == nil {
		return nil
	}
	err := lock.file.Close()
	lock.file = nil
	return err
}

func syncInstallDirectory(root *os.Root, name string) (err error) {
	file, err := root.Open(name)
	if err != nil {
		return err
	}
	defer func() { err = errors.Join(err, file.Close()) }()
	return syncDirectoryFile(file)
}

func writeInstallRecord(root *os.Root, name string, value any) error {
	if info, err := root.Lstat(name); err == nil && !info.Mode().IsRegular() {
		return fmt.Errorf("更新记录目标不是普通文件")
	} else if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	if err := atomicJSON(root, name, value); err != nil {
		return err
	}
	return syncInstallDirectory(root, ".")
}
