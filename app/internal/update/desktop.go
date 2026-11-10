package update

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// Desktop 在 GUI 与全部业务服务结束前持有原数据目录应用锁和安装锁。
// 未完成恢复时不能创建业务服务；committed 备份必须等页面显式确认后清理。
type Desktop struct {
	mu           sync.Mutex
	installOp    sync.Mutex
	preparation  *desktopPreparation
	root         *os.Root
	data         *os.Root
	instance     *installLock
	engine       *installer
	current      BuildInfo
	install      installation
	confirmation string
	recovery     *installJournal
	closed       bool
	closedDone   chan struct{}
	closeErr     error
}

func OpenDesktop(ctx context.Context, dataDirectory, executable string) (*Desktop, error) {
	current, err := CurrentBuild()
	if err != nil {
		return nil, err
	}
	return openDesktop(ctx, dataDirectory, executable, current)
}

func openDesktop(ctx context.Context, dataDirectory, executable string, current BuildInfo) (desktop *Desktop, err error) {
	canonical, err := canonicalUpdatePath(executable)
	if err != nil {
		return nil, err
	}
	root, err := openRoot(filepath.Join(dataDirectory, "updates"))
	if err != nil {
		return nil, err
	}
	d := &Desktop{root: root, current: current, install: recoveredInstallation(canonical)}
	defer func() {
		if err != nil {
			err = errors.Join(err, d.Close())
		}
	}()
	d.data, err = os.OpenRoot(filepath.Dir(root.Name()))
	if err != nil {
		return nil, err
	}
	// 全部启动加锁均不等待；helper 持有任一锁时不让 GUI 进入初始化。
	d.instance, err = acquireInstallLock(ctx, d.data, "kitonyterms.lock", 0)
	if err != nil {
		return nil, fmt.Errorf("该数据目录已由另一个 KitonyTerms 进程使用：%w", err)
	}
	d.engine, err = openInstaller(ctx, root)
	if err != nil {
		return nil, err
	}
	helper, err := acquireInstallLock(ctx, root, "helper.lock", 0)
	if err != nil {
		return nil, fmt.Errorf("更新辅助程序仍在运行：%w", err)
	}
	defer func() { err = errors.Join(err, helper.Close()) }()
	if present, checkErr := markerPresent(root, "task.json"); checkErr != nil {
		return nil, checkErr
	} else if present {
		task, readErr := readInstallTask(root)
		if readErr != nil {
			return nil, readErr
		}
		if !sameRecoveryExecutable(task.Installation.Executable, canonical) {
			return nil, fmt.Errorf("更新任务属于其他应用路径")
		}
		alive, aliveErr := updateProcessAlive(task.ParentPID)
		if aliveErr != nil || alive {
			return nil, errors.Join(aliveErr, fmt.Errorf("更新重启仍在等待旧进程"))
		}
	}
	if present, checkErr := markerPresent(root, "relaunch.json"); checkErr != nil {
		return nil, checkErr
	} else if present {
		record, _, readErr := readRecoveryRelaunch(root)
		if readErr != nil {
			return nil, readErr
		}
		alive, aliveErr := updateProcessAlive(record.ParentPID)
		if aliveErr != nil || alive || !sameRecoveryExecutable(record.Executable, canonical) {
			return nil, errors.Join(aliveErr, fmt.Errorf("恢复重启记录仍在使用或属于其他应用"))
		}
	}
	if present, checkErr := markerPresent(root, "journal.json"); checkErr != nil {
		return nil, checkErr
	} else if present {
		journal, digest, readErr := readInstallJournal(root)
		if readErr != nil {
			return nil, readErr
		}
		if !sameRecoveryExecutable(journal.Task.Installation.Executable, canonical) {
			return nil, fmt.Errorf("安装记录属于其他应用路径")
		}
		if current.Channel == "development" {
			return nil, fmt.Errorf("开发构建不能接管正式更新事务")
		}
		d.install = journal.Task.Installation
		if journal.Stage != "committed" || journal.Task.Next != current {
			d.recovery = &journal
			if err = cleanupRelaunchMarkers(root); err != nil {
				return nil, err
			}
			return d, nil
		}
		if err = d.engine.recover(ctx, current, canonical, false); err != nil {
			return nil, err
		}
		d.confirmation = digest
	}
	if err = cleanupUpdateMarkers(root); err != nil {
		return nil, err
	}
	return d, nil
}

func (d *Desktop) NeedsRecovery() bool {
	d.mu.Lock()
	defer d.mu.Unlock()
	return !d.closed && d.recovery != nil
}

// 调用者尚未启动 GUI/会话；commit 后必须结束本进程，helper 仍等待真实退出。
func (d *Desktop) RelaunchRecovery(ctx context.Context) (err error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.closed || d.recovery == nil {
		return fmt.Errorf("没有可重启恢复的安装事务")
	}
	journal, _, err := readInstallJournal(d.root)
	if err != nil || journal != *d.recovery {
		return errors.Join(err, fmt.Errorf("启动恢复记录已改变"))
	}
	record, err := d.engine.prepareRecoveryRelaunch(ctx, d.install, journal.Task.Current, true)
	if err != nil {
		return err
	}
	process, err := spawnRecoveryRelaunch(ctx, d.root, record)
	if err != nil {
		return errors.Join(err, cleanupRelaunchMarkers(d.root))
	}
	if err = writeInstallRecord(d.root, "commit.json", true); err == nil {
		return nil
	}
	// 旧进程尚未退出，不能留下无人管理的未授权 helper。
	abortErr := writeInstallRecord(d.root, "abort.json", true)
	select {
	case <-process.done:
	case <-time.After(2 * time.Second):
		killErr := process.command.Process.Kill()
		if errors.Is(killErr, os.ErrProcessDone) {
			killErr = nil
		}
		abortErr = errors.Join(abortErr, killErr)
		<-process.done
	}
	return errors.Join(err, abortErr, cleanupRelaunchMarkers(d.root))
}

func (d *Desktop) ConfirmReady(ctx context.Context) error {
	d.mu.Lock()
	defer d.mu.Unlock()
	if d.closed || d.recovery != nil {
		return fmt.Errorf("桌面启动恢复尚未完成")
	}
	if d.confirmation == "" {
		return nil
	}
	journal, digest, err := readInstallJournal(d.root)
	if err != nil || digest != d.confirmation || journal.Stage != "committed" || journal.Task.Next != d.current || !sameRecoveryExecutable(journal.Task.Installation.Executable, d.install.Executable) {
		return errors.Join(err, fmt.Errorf("页面就绪前安装记录已改变，保留恢复材料"))
	}
	if err = d.engine.recover(ctx, d.current, d.install.Executable, true); err != nil {
		return err
	}
	d.confirmation = ""
	return nil
}

func (d *Desktop) Close() error {
	if d == nil {
		return nil
	}
	d.mu.Lock()
	if d.closed {
		done := d.closedDone
		d.mu.Unlock()
		<-done
		return d.closeErr
	}
	d.closed = true
	d.closedDone = make(chan struct{})
	if d.preparation != nil {
		d.preparation.cancel()
	}
	d.mu.Unlock()
	d.installOp.Lock()
	defer d.installOp.Unlock()
	var err error
	if d.preparation != nil && !d.preparation.committed {
		err = d.abortPreparation(d.preparation)
	}
	if d.engine != nil {
		err = errors.Join(err, d.engine.Close())
	}
	err = errors.Join(err, d.instance.Close())
	if d.root != nil {
		err = errors.Join(err, d.root.Close())
	}
	if d.data != nil {
		err = errors.Join(err, d.data.Close())
	}
	d.closeErr = err
	close(d.closedDone)
	return err
}
