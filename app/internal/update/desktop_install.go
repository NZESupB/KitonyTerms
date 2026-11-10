package update

import (
	"context"
	"errors"
	"fmt"
	"os"
	"runtime"
	"time"
)

type desktopPreparation struct {
	id        string
	cancel    context.CancelFunc
	task      *installTask
	process   *updateHelperProcess
	ready     bool
	committed bool
}

// installOp 串行化引擎和协议文件；mu 只保护准入与取消，不覆盖长时间准备。
func (d *Desktop) prepareInstall(ctx context.Context, id string, expected installation, candidate available) (err error) {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	p := &desktopPreparation{id: id, cancel: cancel}
	d.mu.Lock()
	executable := d.install.Executable
	if id == "" || d.closed || d.preparation != nil || d.recovery != nil || d.confirmation != "" || d.current.Channel == "development" || d.current.Target == "" {
		d.mu.Unlock()
		return fmt.Errorf("桌面更新当前不能准备安装")
	}
	d.preparation = p
	d.mu.Unlock()
	d.installOp.Lock()
	defer d.installOp.Unlock()
	defer func() {
		if err != nil {
			err = errors.Join(err, d.abortPreparation(p))
		}
	}()
	d.mu.Lock()
	active := !d.closed && d.preparation == p
	d.mu.Unlock()
	if !active || ctx.Err() != nil {
		return errors.Join(ctx.Err(), fmt.Errorf("安装准备已取消"))
	}
	install, err := discoverInstallation(executable)
	if err != nil {
		return err
	}
	if install != expected {
		return fmt.Errorf("安装方式已改变，请重新检查并下载更新")
	}
	access := assessInstallation(ctx, d.root, install, runtime.GOOS)
	if access.Reason != "" {
		return errors.Join(access.Err, fmt.Errorf("安装目录当前不可更新"))
	}
	if err = requireCleanHelperMarkers(d.root); err != nil {
		return err
	}
	task, err := d.engine.prepareTask(ctx, install, d.current, candidate, access.Elevate)
	if task.ParentPID == uint32(os.Getpid()) && validHex(task.SourceSHA256, 64) {
		p.task = &task
	}
	if err != nil {
		return err
	}
	if err = d.engine.installHelper(ctx, task); err != nil {
		return err
	}
	p.process, err = spawnApplyHelper(ctx, d.root, task.SourceSHA256)
	if err != nil {
		return err
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	p.ready = true
	return nil
}

func (d *Desktop) commitInstall(ctx context.Context, id string) error {
	d.installOp.Lock()
	defer d.installOp.Unlock()
	d.mu.Lock()
	p := d.preparation
	closed := d.closed
	d.mu.Unlock()
	if closed || p == nil || id == "" || p.id != id || !p.ready || p.task == nil || p.process == nil {
		return fmt.Errorf("安装准备请求已失效")
	}
	if p.committed {
		return nil
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	select {
	case <-p.process.done:
		return errors.Join(p.process.err, fmt.Errorf("更新辅助程序已退出，请重新准备"))
	default:
	}
	if err := checkPreparationRecords(d.root, *p.task, uint32(p.process.command.Process.Pid)); err != nil {
		return err
	}
	if err := readHelperReady(d.root, uint32(p.process.command.Process.Pid), p.task.SourceSHA256); err != nil {
		return err
	}
	for _, name := range []string{"commit.json", "abort.json"} {
		if present, err := markerPresent(d.root, name); err != nil || present {
			return errors.Join(err, fmt.Errorf("安装授权标记已改变"))
		}
	}
	if err := verifyRestoredInstall(ctx, d.root, *p.task); err != nil {
		return err
	}
	if err := writeInstallRecord(d.root, "commit.json", true); err != nil {
		return err
	}
	p.committed = true
	return nil
}

func (d *Desktop) abortInstall(id string) error {
	d.mu.Lock()
	p := d.preparation
	if p == nil {
		d.mu.Unlock()
		return nil
	}
	if id == "" || p.id != id {
		d.mu.Unlock()
		return fmt.Errorf("安装准备请求已失效")
	}
	p.cancel()
	d.mu.Unlock()
	d.installOp.Lock()
	defer d.installOp.Unlock()
	return d.abortPreparation(p)
}

// 调用者持有 installOp。外来任务或任何 journal 都保留；仅回收本次 helper。
func (d *Desktop) abortPreparation(p *desktopPreparation) error {
	d.mu.Lock()
	active := d.preparation == p
	d.mu.Unlock()
	if !active {
		return nil
	}
	if p.committed {
		return fmt.Errorf("安装已经提交，正在等待应用退出")
	}
	var err error
	if p.task != nil {
		pid := uint32(0)
		if p.process != nil {
			pid = uint32(p.process.command.Process.Pid)
		}
		if present, checkErr := markerPresent(d.root, "task.json"); checkErr != nil {
			err = checkErr
		} else if present {
			if checkErr = checkPreparationRecords(d.root, *p.task, pid); checkErr == nil {
				err = writeInstallRecord(d.root, "abort.json", true)
			} else {
				err = checkErr
			}
		}
	}
	if p.process != nil {
		timer := time.NewTimer(2 * time.Second)
		select {
		case <-p.process.done:
			timer.Stop()
		case <-timer.C:
			killErr := p.process.command.Process.Kill()
			if errors.Is(killErr, os.ErrProcessDone) {
				killErr = nil
			}
			err = errors.Join(err, killErr)
			<-p.process.done
		}
	}
	if p.task != nil {
		pid := uint32(0)
		if p.process != nil {
			pid = uint32(p.process.command.Process.Pid)
		}
		err = errors.Join(err, cleanupPreparationRecords(d.root, *p.task, pid))
	}
	p.ready = false
	d.mu.Lock()
	if d.preparation == p && err == nil {
		d.preparation = nil
	}
	d.mu.Unlock()
	return err
}

func checkPreparationRecords(root *os.Root, task installTask, pid uint32) error {
	for _, name := range []string{"journal.json", "relaunch.json"} {
		if present, err := markerPresent(root, name); err != nil || present {
			return errors.Join(err, fmt.Errorf("安装准备记录已改变，保留恢复材料"))
		}
	}
	persisted, err := readInstallTask(root)
	if err != nil || persisted != task {
		return errors.Join(err, errTaskMismatch)
	}
	if present, err := markerPresent(root, "helper-ready.json"); err != nil {
		return err
	} else if present {
		if err = readHelperReady(root, pid, task.SourceSHA256); err != nil {
			return err
		}
	}
	for _, name := range []string{"commit.json", "abort.json"} {
		if present, err := markerPresent(root, name); err != nil {
			return err
		} else if present {
			data, err := readPrivate(root, name, 1024)
			var flag bool
			if err != nil || decodeJSON(data, &flag, true) != nil || !flag {
				return errors.Join(err, fmt.Errorf("安装准备授权标记无效"))
			}
		}
	}
	return nil
}

func cleanupPreparationRecords(root *os.Root, task installTask, pid uint32) error {
	if present, err := markerPresent(root, "task.json"); err != nil {
		return err
	} else if !present {
		// helper 可能已自行清理；缺任务但仍有其它记录时不能猜测归属。
		if err = pendingInstallation(root); err != nil {
			return err
		}
		return requireCleanHelperMarkers(root)
	}
	if err := checkPreparationRecords(root, task, pid); err != nil {
		return err
	}
	return cleanupUpdateMarkers(root)
}
