package session

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"strconv"
	"sync"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
	"kitonyterms/internal/remoteops"
)

type containerTerminal struct {
	mu                             sync.Mutex
	inputMu                        sync.Mutex
	resizeMu                       sync.Mutex
	ctx                            context.Context
	cancel                         context.CancelFunc
	done                           chan struct{}
	id, container, status, failure string
	released                       bool
	host                           *ssh.Client
	channel                        ssh.Channel
	output                         *outputBuffer
	inputSequence                  uint64
	inputDigest                    string
}

func (c *containerTerminal) snapshot() model.ContainerTerminalSnapshot {
	c.inputMu.Lock()
	defer c.inputMu.Unlock()
	c.mu.Lock()
	defer c.mu.Unlock()
	return model.ContainerTerminalSnapshot{ExecID: c.id, Container: c.container, Status: c.status, Error: c.failure, InputSequence: strconv.FormatUint(c.inputSequence, 10)}
}

// 已接受的终端跟随宿主生命周期；抽屉卸载不取消 PTY，关闭由 ExecID 精确寻址。
func (m *Manager) StartContainerTerminal(id, generation string, r model.ContainerTerminalRequest) (model.ContainerTerminalSnapshot, error) {
	command, err := remoteops.ContainerCommand(r.ExecID, r.Container)
	if err != nil {
		return model.ContainerTerminalSnapshot{}, err
	}
	if !terminalSize(r.Cols, r.Rows) {
		return model.ContainerTerminalSnapshot{}, fmt.Errorf("终端尺寸无效")
	}
	s, err := m.Get(id, generation)
	if err != nil {
		return model.ContainerTerminalSnapshot{}, err
	}
	m.mu.Lock()
	if m.closed || m.draining != "" {
		m.mu.Unlock()
		return model.ContainerTerminalSnapshot{}, fmt.Errorf("应用正在关闭")
	}
	s.mu.Lock()
	if s.client == nil || s.status != "connected" || s.ctx.Err() != nil {
		s.mu.Unlock()
		m.mu.Unlock()
		return model.ContainerTerminalSnapshot{}, fmt.Errorf("SSH 会话未连接")
	}
	if previous := s.container; previous != nil {
		if previous.id == r.ExecID {
			s.mu.Unlock()
			m.mu.Unlock()
			if previous.container != r.Container {
				return model.ContainerTerminalSnapshot{}, fmt.Errorf("容器终端 ID 已用于其它目标")
			}
			return previous.snapshot(), nil
		}
		select {
		case <-previous.done:
		default:
			s.mu.Unlock()
			m.mu.Unlock()
			return model.ContainerTerminalSnapshot{}, fmt.Errorf("请先关闭当前容器终端")
		}
	}
	ctx, cancel := context.WithCancel(s.ctx)
	c := &containerTerminal{ctx: ctx, cancel: cancel, done: make(chan struct{}), id: r.ExecID, container: r.Container, status: "starting", host: s.client, output: newOutput()}
	s.container = c
	m.addWorkerLocked()
	s.mu.Unlock()
	m.mu.Unlock()
	go func() {
		defer m.workerDone()
		defer close(c.done)
		err := c.run(command, r.Cols, r.Rows)
		c.cancel()
		c.mu.Lock()
		c.channel = nil
		c.status = "closed"
		if err != nil && !errors.Is(err, context.Canceled) {
			c.failure = err.Error()
		}
		failure := c.failure
		c.mu.Unlock()
		c.output.close(failure)
	}()
	return c.snapshot(), nil
}

func terminalSize(cols, rows int) bool { return cols >= 2 && cols <= 1000 && rows >= 1 && rows <= 500 }

func (m *Manager) containerTerminal(id, generation, execID string) (*containerTerminal, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.container == nil || s.container.id != execID {
		return nil, fmt.Errorf("容器终端或连接代次已失效")
	}
	return s.container, nil
}
func (m *Manager) GetContainerTerminal(id, generation, execID string) (model.ContainerTerminalSnapshot, error) {
	if execID == "" {
		s, err := m.Get(id, generation)
		if err != nil {
			return model.ContainerTerminalSnapshot{}, err
		}
		s.mu.Lock()
		current := s.container
		s.mu.Unlock()
		if current == nil {
			return model.ContainerTerminalSnapshot{}, nil
		}
		current.mu.Lock()
		released := current.released
		current.mu.Unlock()
		if released {
			return model.ContainerTerminalSnapshot{}, nil
		}
		return current.snapshot(), nil
	}
	c, err := m.containerTerminal(id, generation, execID)
	if err != nil {
		return model.ContainerTerminalSnapshot{}, err
	}
	return c.snapshot(), nil
}
func (m *Manager) PollContainerTerminal(id, generation, execID, after string) (model.TerminalBatch, error) {
	c, err := m.containerTerminal(id, generation, execID)
	if err != nil {
		return model.TerminalBatch{}, err
	}
	return c.output.poll(after)
}
func (m *Manager) CloseContainerTerminal(id, generation, execID string) error {
	c, err := m.containerTerminal(id, generation, execID)
	if err != nil {
		return err
	}
	c.cancel()
	<-c.done
	c.mu.Lock()
	c.released = true
	c.mu.Unlock()
	return nil
}
func (m *Manager) SendContainerInput(ctx context.Context, r model.ContainerInputRequest) error {
	if len(r.Data) > base64.StdEncoding.EncodedLen(64<<10) {
		return fmt.Errorf("终端输入长度无效")
	}
	data, err := base64.StdEncoding.DecodeString(r.Data)
	if err != nil || len(data) > 64<<10 {
		return fmt.Errorf("终端输入编码或长度无效")
	}
	sequence, err := strconv.ParseUint(r.Sequence, 10, 64)
	if err != nil || sequence == 0 {
		return fmt.Errorf("终端输入序号无效")
	}
	c, err := m.containerTerminal(r.SessionID, r.Generation, r.ExecID)
	if err != nil {
		return err
	}
	c.inputMu.Lock()
	defer c.inputMu.Unlock()
	c.mu.Lock()
	ch, status := c.channel, c.status
	c.mu.Unlock()
	if ch == nil || status != "running" || c.ctx.Err() != nil {
		return fmt.Errorf("容器终端尚未连接或已关闭")
	}
	if sequence == c.inputSequence && c.inputDigest == digest(data) {
		return nil
	}
	if sequence != c.inputSequence+1 {
		return fmt.Errorf("终端输入序号不连续")
	}
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	stop := context.AfterFunc(c.ctx, cancel)
	defer stop()
	_, err = negotiation(ctx, c.host, ch, func() (int, error) {
		n, e := ch.Write(data)
		if e == nil && n != len(data) {
			e = io.ErrShortWrite
		}
		return n, e
	})
	if err != nil {
		c.cancel()
		return fmt.Errorf("容器终端输入失败: %w", err)
	}
	c.inputSequence, c.inputDigest = sequence, digest(data)
	return nil
}
func (m *Manager) ResizeContainerTerminal(ctx context.Context, id, generation, execID string, cols, rows int) error {
	if !terminalSize(cols, rows) {
		return fmt.Errorf("终端尺寸无效")
	}
	c, err := m.containerTerminal(id, generation, execID)
	if err != nil {
		return err
	}
	c.resizeMu.Lock()
	defer c.resizeMu.Unlock()
	c.mu.Lock()
	ch, status := c.channel, c.status
	c.mu.Unlock()
	if status == "starting" {
		return nil
	}
	if ch == nil || status != "running" || c.ctx.Err() != nil {
		return fmt.Errorf("容器终端已关闭")
	}
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	stop := context.AfterFunc(c.ctx, cancel)
	defer stop()
	_, err = negotiation(ctx, c.host, ch, func() (bool, error) {
		return ch.SendRequest("window-change", false, ssh.Marshal(struct{ Cols, Rows, Width, Height uint32 }{uint32(cols), uint32(rows), 0, 0}))
	})
	return err
}

func (c *containerTerminal) run(command string, cols, rows int) error {
	opening, cancel := context.WithTimeout(c.ctx, 15*time.Second)
	defer cancel()
	type opened struct {
		ch       ssh.Channel
		requests <-chan *ssh.Request
	}
	result, err := negotiation(opening, c.host, nil, func() (opened, error) {
		ch, requests, e := c.host.OpenChannel("session", nil)
		return opened{ch, requests}, e
	})
	if err != nil {
		if result.ch != nil {
			closeOperationChannel(c.host, result.ch)
		}
		return fmt.Errorf("打开容器终端通道: %w", err)
	}
	ch := result.ch
	stopDone := make(chan struct{})
	stop := context.AfterFunc(c.ctx, func() { closeOperationChannel(c.host, ch); close(stopDone) })
	type exit struct {
		status  uint32
		present bool
	}
	exited := make(chan exit, 1)
	go func() {
		value := exit{}
		for r := range result.requests {
			if r.Type == "exit-status" {
				var payload struct{ Status uint32 }
				if ssh.Unmarshal(r.Payload, &payload) == nil {
					value = exit{payload.Status, true}
				}
			}
			if r.WantReply {
				_ = r.Reply(false, nil)
			}
		}
		exited <- value
	}()
	var streams sync.WaitGroup
	streams.Add(2)
	pump := func(reader io.Reader) {
		defer streams.Done()
		buffer := make([]byte, 32768)
		for {
			n, e := reader.Read(buffer)
			if n > 0 {
				if err := c.output.append(c.ctx, buffer[:n]); err != nil {
					return
				}
			}
			if e != nil {
				return
			}
		}
	}
	go pump(ch)
	go pump(ch.Stderr())
	drained := make(chan struct{})
	go func() { streams.Wait(); close(drained) }()
	requestsDone, streamsDone := false, false
	defer func() {
		c.cancel()
		if stop() {
			closeOperationChannel(c.host, ch)
		} else {
			<-stopDone
		}
		timer := time.AfterFunc(500*time.Millisecond, func() { _ = c.host.Close() })
		defer timer.Stop()
		if !streamsDone {
			<-drained
		}
		if !requestsDone {
			<-exited
		}
	}()
	for _, request := range []struct {
		name    string
		payload []byte
	}{
		{"pty-req", ssh.Marshal(struct {
			Term                      string
			Cols, Rows, Width, Height uint32
			Modes                     string
		}{"xterm-256color", uint32(cols), uint32(rows), 0, 0, string([]byte{0})})},
		{"exec", ssh.Marshal(struct{ Command string }{command})},
	} {
		_, err = negotiation(opening, c.host, ch, func() (bool, error) {
			ok, e := ch.SendRequest(request.name, true, request.payload)
			if e == nil && !ok {
				e = fmt.Errorf("远端拒绝容器终端 %s 请求", request.name)
			}
			return ok, e
		})
		if err != nil {
			return err
		}
	}
	cancel()
	if err = c.ctx.Err(); err != nil {
		return err
	}
	c.mu.Lock()
	c.channel, c.status = ch, "running"
	c.mu.Unlock()
	var status exit
	requests, output := exited, drained
	var drainTimer *time.Timer
	defer func() {
		if drainTimer != nil {
			drainTimer.Stop()
		}
	}()
	var drainLimit <-chan time.Time
	for !requestsDone || !streamsDone {
		select {
		case <-c.ctx.Done():
			return c.ctx.Err()
		case status = <-requests:
			requestsDone = true
			requests = nil
			if !streamsDone {
				drainTimer = time.NewTimer(500 * time.Millisecond)
				drainLimit = drainTimer.C
			}
		case <-output:
			streamsDone = true
			output = nil
		case <-drainLimit:
			return fmt.Errorf("容器终端关闭时输出未在 500ms 内消费，已保留缓冲区内容")
		}
	}
	if !status.present {
		return fmt.Errorf("容器终端未返回退出状态")
	}
	if status.status != 0 {
		return fmt.Errorf("容器终端退出（状态 %d）", status.status)
	}
	return nil
}
