package lifecycle

import (
	"context"
	"crypto/rand"
	"fmt"
	"sync"
)

type State struct {
	RequestID           string
	Intent              string
	Preparing           bool
	Ready               bool
	Committing          bool
	Cancelling          bool
	ProtectedOperations int
}

type InstallRequest struct {
	ID      string
	Context context.Context
	cancel  context.CancelFunc
	done    chan struct{}
	once    sync.Once
}

func (r *InstallRequest) Prepared()     { r.once.Do(func() { close(r.done) }) }
func (r *InstallRequest) WaitPrepared() { <-r.done }

// 控制器将退出意图与业务准入放在同一锁内；业务清理完成后才放行原生退出。
type Controller struct {
	mu         sync.Mutex
	enabled    bool
	allowed    bool
	state      State
	operations int
	install    *InstallRequest
	notify     func()
}

func New(notify func()) *Controller { return &Controller{notify: notify} }
func (c *Controller) Enable()       { c.mu.Lock(); c.enabled = true; c.mu.Unlock() }
func (c *Controller) Snapshot() State {
	c.mu.Lock()
	defer c.mu.Unlock()
	state := c.state
	state.ProtectedOperations = c.operations
	return state
}
func (c *Controller) ShouldQuit() bool {
	c.mu.Lock()
	if !c.enabled || c.allowed {
		c.mu.Unlock()
		return true
	}
	if c.state.RequestID == "" {
		c.state = State{RequestID: rand.Text(), Intent: "quit"}
	}
	notify := c.notify
	c.mu.Unlock()
	if notify != nil {
		notify()
	}
	return false
}
func (c *Controller) Cancel(id string) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if id == "" || id != c.state.RequestID || c.state.Intent != "quit" {
		return fmt.Errorf("退出请求已失效")
	}
	if c.state.Preparing {
		return fmt.Errorf("应用正在退出")
	}
	c.state = State{}
	return nil
}
func (c *Controller) Begin(id string) (bool, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if id == "" || id != c.state.RequestID || c.state.Intent != "quit" {
		return false, fmt.Errorf("退出请求已失效")
	}
	if c.state.Preparing {
		return false, nil
	}
	if c.operations != 0 {
		return false, fmt.Errorf("仍有文件选择、保存或同步操作，请等待完成")
	}
	c.state.Preparing = true
	return true, nil
}
func (c *Controller) Complete(id string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if id == "" || id != c.state.RequestID || !c.state.Preparing || c.state.Intent != "quit" {
		return false
	}
	c.allowed = true
	return true
}

// 只记录活动数量，不接收文件内容、凭据、远端命令或用户输入。
func (c *Controller) BeginOperation() (func(), error) {
	c.mu.Lock()
	if c.allowed || c.state.Preparing {
		c.mu.Unlock()
		return nil, fmt.Errorf("应用正在准备退出")
	}
	c.operations++
	c.mu.Unlock()
	var once sync.Once
	return func() {
		once.Do(func() {
			c.mu.Lock()
			c.operations--
			c.mu.Unlock()
		})
	}, nil
}

func (c *Controller) StartInstall(parent context.Context) (*InstallRequest, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if err := parent.Err(); err != nil {
		return nil, err
	}
	if !c.enabled || c.allowed || c.state.RequestID != "" {
		return nil, fmt.Errorf("已有退出或安装请求")
	}
	if c.operations != 0 {
		return nil, fmt.Errorf("仍有文件选择、保存或同步操作，请等待完成")
	}
	ctx, cancel := context.WithCancel(parent)
	r := &InstallRequest{ID: rand.Text(), Context: ctx, cancel: cancel, done: make(chan struct{})}
	c.install = r
	c.state = State{RequestID: r.ID, Intent: "install", Preparing: true}
	return r, nil
}
func (c *Controller) ReadyInstall(id string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if id == "" || id != c.state.RequestID || c.state.Intent != "install" || c.state.Cancelling || c.state.Committing || c.allowed || c.install.Context.Err() != nil {
		return false
	}
	c.state.Ready = true
	return true
}
func (c *Controller) BeginInstallCommit(id string) (bool, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if id == "" || id != c.state.RequestID || c.state.Intent != "install" || !c.state.Ready || c.state.Cancelling {
		return false, fmt.Errorf("安装准备请求已失效或尚未就绪")
	}
	if c.state.Committing {
		return false, nil
	}
	c.state.Committing = true
	return true, nil
}

// 持有退出所有权直到提交返回；成功时与放行原生退出原子完成。
func (c *Controller) AuthorizeInstall(id string, commit func() error) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	if id == "" || id != c.state.RequestID || c.state.Intent != "install" || !c.state.Committing || c.state.Cancelling {
		return fmt.Errorf("安装提交请求已失效")
	}
	if c.allowed {
		return nil
	}
	if err := commit(); err != nil {
		return err
	}
	c.allowed = true
	c.install.cancel()
	return nil
}
func (c *Controller) BeginInstallAbort(id string) (*InstallRequest, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if id == "" || id != c.state.RequestID || c.state.Intent != "install" || c.state.Committing || c.allowed {
		return nil, fmt.Errorf("安装准备请求已失效或正在提交")
	}
	c.state.Cancelling = true
	c.install.cancel()
	return c.install, nil
}

// 仅供业务层在未提交或提交失败且完成回收后调用；旧请求不能清除新意图。
func (c *Controller) FailInstall(id string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	if id == "" || id != c.state.RequestID || c.state.Intent != "install" || c.allowed {
		return false
	}
	c.install.cancel()
	c.install = nil
	c.state = State{}
	return true
}
