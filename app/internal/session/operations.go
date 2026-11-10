package session

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"golang.org/x/crypto/ssh"
	"io"
	"kitonyterms/internal/model"
	"kitonyterms/internal/remoteops"
	"strings"
	"sync"
	"time"
	"unicode/utf8"
)

type operationRun struct {
	cancel    context.CancelFunc
	challenge *model.OperationChallenge
	answer    chan model.OperationAnswer
	done      chan struct{}
}

type operationOutput struct {
	mu       sync.Mutex
	data     []byte
	limit    int
	overflow bool
	cancel   context.CancelFunc
	prompt   chan struct{}
	prompted bool
}

func (o *operationOutput) Write(data []byte) (int, error) {
	o.mu.Lock()
	defer o.mu.Unlock()
	if len(data) > o.limit-len(o.data) {
		o.overflow = true
		o.cancel()
		return 0, remoteops.Failure("output_limit", "运维输出超过限制")
	}
	o.data = append(o.data, data...)
	if o.prompt != nil && !o.prompted && bytes.Contains(o.data, []byte(remoteops.SudoPrompt)) {
		o.prompted = true
		o.prompt <- struct{}{}
	}
	return len(data), nil
}
func (o *operationOutput) snapshot() (string, bool) {
	o.mu.Lock()
	defer o.mu.Unlock()
	return string(o.data), o.overflow
}

func closeOperationChannel(host *ssh.Client, ch ssh.Channel) {
	done := make(chan struct{})
	go func() { _ = ch.Close(); close(done) }()
	timer := time.NewTimer(500 * time.Millisecond)
	defer timer.Stop()
	select {
	case <-done:
		return
	case <-timer.C:
		_ = host.Close()
		<-done
	}
}

// 操作、挑战及结果归属于同一连接代次；正常取消仅关闭本次 exec 通道。
func (m *Manager) ExecuteOperation(ctx context.Context, id, generation string, r model.OperationRequest) (reply model.OperationReply, err error) {
	reply = model.OperationReply{RequestID: r.ID, Kind: r.Kind}
	defer func() {
		if err != nil {
			var failure *model.OperationError
			if !errors.As(err, &failure) {
				failure = remoteops.Failure("command_failed", "运维请求失败")
			}
			reply = model.OperationReply{RequestID: r.ID, Kind: r.Kind, Error: failure}
			err = nil
		}
	}()
	command, err := remoteops.Command(r)
	if err != nil {
		return reply, err
	}
	s, err := m.Get(id, generation)
	if err != nil {
		return reply, remoteops.Failure("disconnected", "会话或连接代次已失效")
	}
	timeout := 12 * time.Second
	if remoteops.IsAction(r) {
		timeout = 60 * time.Second
	}
	if r.Kind == "docker_storage" || r.Kind == "docker_image_sizes" {
		timeout = 45 * time.Second
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	stop := context.AfterFunc(s.ctx, cancel)
	defer stop()
	run := &operationRun{cancel: cancel, answer: make(chan model.OperationAnswer, 1), done: make(chan struct{})}
	m.mu.Lock()
	if m.closed || m.draining != "" {
		m.mu.Unlock()
		return reply, remoteops.Failure("disconnected", "应用正在关闭")
	}
	m.addWorkerLocked()
	m.mu.Unlock()
	defer m.workerDone()
	s.mu.Lock()
	if s.client == nil || s.status != "connected" || s.ctx.Err() != nil {
		s.mu.Unlock()
		return reply, remoteops.Failure("disconnected", "SSH 会话未连接")
	}
	if len(s.operations) >= 4 || s.operations[r.ID] != nil {
		s.mu.Unlock()
		return reply, remoteops.Failure("busy", "当前运维请求过多或操作 ID 重复")
	}
	if s.operations == nil {
		s.operations = map[string]*operationRun{}
	}
	s.operations[r.ID] = run
	host := s.client
	caps := s.capabilities
	s.mu.Unlock()
	defer close(run.done)
	defer func() {
		s.mu.Lock()
		delete(s.operations, r.ID)
		run.challenge = nil
		select {
		case a := <-run.answer:
			a.Password = ""
		default:
		}
		s.mu.Unlock()
	}()
	if ctx.Err() != nil {
		return reply, operationContextError(ctx, s)
	}
	if r.Kind != "capabilities" && r.Kind != "status" {
		if caps == nil {
			text, e := m.runOperation(ctx, s, host, run, r.ID, remoteops.ProbeCommand, false)
			if e != nil {
				return reply, e
			}
			parsed, e := remoteops.Parse(model.OperationRequest{ID: r.ID, Kind: "capabilities"}, text)
			if e != nil {
				return reply, e
			}
			caps = parsed.Capabilities
			s.mu.Lock()
			s.capabilities = caps
			s.mu.Unlock()
		}
		if !remoteops.Supported(*caps, r) {
			return reply, remoteops.Failure("unsupported_backend", "此服务器尚未提供该功能的可用后端，请重新探测能力")
		}
	}
	text, err := m.runOperation(ctx, s, host, run, r.ID, command, remoteops.IsAction(r))
	if err != nil {
		return reply, err
	}
	reply, err = remoteops.Parse(r, text)
	if err == nil && reply.Capabilities != nil {
		s.mu.Lock()
		s.capabilities = reply.Capabilities
		s.mu.Unlock()
	}
	return reply, err
}
func operationContextError(ctx context.Context, s *Session) error {
	if s.ctx.Err() != nil {
		return remoteops.Failure("disconnected", "SSH 会话已断开")
	}
	if errors.Is(ctx.Err(), context.DeadlineExceeded) {
		return remoteops.Failure("timeout", "远程运维请求超时")
	}
	return remoteops.Failure("cancelled", "运维请求已取消")
}
func (m *Manager) runOperation(ctx context.Context, s *Session, host *ssh.Client, run *operationRun, id, command string, action bool) (string, error) {
	type opened struct {
		ch       ssh.Channel
		requests <-chan *ssh.Request
	}
	openedChannel, err := negotiation(ctx, host, nil, func() (opened, error) {
		ch, requests, e := host.OpenChannel("session", nil)
		return opened{ch, requests}, e
	})
	if err != nil {
		if openedChannel.ch != nil {
			closeOperationChannel(host, openedChannel.ch)
		}
		if ctx.Err() != nil {
			return "", operationContextError(ctx, s)
		}
		return "", remoteops.Failure("disconnected", "无法打开 SSH 运维通道")
	}
	ch := openedChannel.ch
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	prompt := make(chan struct{}, 1)
	stdout := &operationOutput{limit: 8 << 20, cancel: cancel}
	stderr := &operationOutput{limit: 64 << 10, cancel: cancel}
	if action {
		stderr.prompt = prompt
	}
	var streams sync.WaitGroup
	streams.Add(2)
	pump := func(dst io.Writer, src io.Reader) { defer streams.Done(); _, _ = io.Copy(dst, src) }
	go pump(stdout, ch)
	go pump(stderr, ch.Stderr())
	type exit struct {
		status  uint32
		present bool
	}
	exitChannel := make(chan exit, 1)
	go func() {
		value := exit{}
		for req := range openedChannel.requests {
			if req.Type == "exit-status" {
				var status struct{ Status uint32 }
				if ssh.Unmarshal(req.Payload, &status) == nil {
					value = exit{status.Status, true}
				}
			}
			if req.WantReply {
				_ = req.Reply(false, nil)
			}
		}
		exitChannel <- value
	}()
	requestsDone, streamsDone := false, false
	drained := make(chan struct{})
	go func() { streams.Wait(); close(drained) }()
	defer func() {
		closeOperationChannel(host, ch)
		if !streamsDone {
			timer := time.AfterFunc(500*time.Millisecond, func() { _ = host.Close() })
			<-drained
			timer.Stop()
		}
		if !requestsDone {
			timer := time.AfterFunc(500*time.Millisecond, func() { _ = host.Close() })
			<-exitChannel
			timer.Stop()
		}
	}()
	_, err = negotiation(ctx, host, ch, func() (bool, error) {
		ok, e := ch.SendRequest("exec", true, ssh.Marshal(struct{ Command string }{command}))
		if e == nil && !ok {
			e = remoteops.Failure("command_failed", "远端拒绝执行运维命令")
		}
		return ok, e
	})
	if err != nil {
		if ctx.Err() != nil {
			return "", operationContextError(ctx, s)
		}
		return "", remoteops.Failure("command_failed", "远端拒绝执行运维命令")
	}
	if !action {
		_ = ch.CloseWrite()
	}
	var status exit
	for !requestsDone || !streamsDone {
		select {
		case <-ctx.Done():
			_, a := stdout.snapshot()
			_, b := stderr.snapshot()
			if a || b {
				return "", remoteops.Failure("output_limit", "远程输出超过限制（正文 8 MiB / 错误 64 KiB）")
			}
			return "", operationContextError(ctx, s)
		case <-prompt:
			prompt = nil
			if err = m.answerSudo(ctx, s, run, id, host, ch); err != nil {
				return "", err
			}
		case status = <-exitChannel:
			requestsDone = true
			exitChannel = nil
		case <-drained:
			streamsDone = true
			drained = nil
		}
	}
	if ctx.Err() != nil {
		return "", operationContextError(ctx, s)
	}
	text, overflow := stdout.snapshot()
	errorText, errorOverflow := stderr.snapshot()
	if overflow || errorOverflow {
		return "", remoteops.Failure("output_limit", "运维输出超过限制")
	}
	if !status.present {
		return "", remoteops.Failure("command_failed", "远程命令未返回退出状态")
	}
	if status.status != 0 {
		return "", remoteops.CommandFailure(status.status, errorText)
	}
	if action {
		return "", nil
	}
	if !utf8.ValidString(text) {
		return "", remoteops.Failure("parse_failed", "远程查询返回无效 UTF-8")
	}
	return text, nil
}
func (m *Manager) answerSudo(ctx context.Context, s *Session, run *operationRun, id string, host *ssh.Client, ch ssh.Channel) error {
	challenge := &model.OperationChallenge{OperationID: id, ID: randomID(), Message: "需要 sudo 密码才能执行此操作"}
	s.mu.Lock()
	run.challenge = challenge
	s.mu.Unlock()
	defer func() { s.mu.Lock(); run.challenge = nil; s.mu.Unlock() }()
	timer := time.NewTimer(45 * time.Second)
	defer timer.Stop()
	var answer model.OperationAnswer
	select {
	case <-ctx.Done():
		return operationContextError(ctx, s)
	case <-timer.C:
		return remoteops.Failure("timeout", "sudo 认证等待超时")
	case answer = <-run.answer:
	}
	if answer.Cancel {
		return remoteops.Failure("cancelled", "sudo 认证已取消")
	}
	password := []byte(answer.Password + "\n")
	answer.Password = ""
	defer clear(password)
	_, err := negotiation(ctx, host, ch, func() (int, error) {
		n, e := ch.Write(password)
		if e == nil && n != len(password) {
			e = io.ErrShortWrite
		}
		return n, e
	})
	if err != nil {
		if ctx.Err() != nil {
			return operationContextError(ctx, s)
		}
		return remoteops.Failure("disconnected", "sudo 凭据无法发送")
	}
	_ = ch.CloseWrite()
	return nil
}
func (m *Manager) OperationChallenges(id, generation string) ([]model.OperationChallenge, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	out := []model.OperationChallenge{}
	for _, run := range s.operations {
		if run.challenge != nil {
			out = append(out, *run.challenge)
		}
	}
	return out, nil
}
func (m *Manager) AnswerOperation(a model.OperationAnswer) error {
	if len(a.Password) > 65536 || strings.ContainsAny(a.Password, "\r\n\x00") {
		return fmt.Errorf("sudo 凭据格式无效")
	}
	s, err := m.Get(a.SessionID, a.Generation)
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	run := s.operations[a.OperationID]
	if run == nil || run.challenge == nil || run.challenge.ID != a.ChallengeID {
		return fmt.Errorf("sudo 认证挑战已失效")
	}
	select {
	case run.answer <- a:
		run.challenge = nil
		return nil
	default:
		return fmt.Errorf("sudo 认证答案已提交")
	}
}
func (m *Manager) CancelOperation(id, generation, operationID string) error {
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	s.mu.Lock()
	run := s.operations[operationID]
	if run != nil {
		run.cancel()
	}
	s.mu.Unlock()
	if run != nil {
		<-run.done
	}
	return nil
}

// 旧只读调用保留兼容，所有执行复用类型化通道及取消路径。
func (m *Manager) RunOperation(ctx context.Context, id, generation, kind, target string) (model.OperationResult, error) {
	if target != "" || !strings.Contains("|status|services|processes|network|docker|", "|"+kind+"|") {
		return model.OperationResult{}, fmt.Errorf("不支持的运维查询")
	}
	r := model.OperationRequest{ID: randomID(), Kind: kind}
	if kind == "docker" {
		r.Kind = "docker_list"
		r.Resource = "containers"
	}
	reply, err := m.ExecuteOperation(ctx, id, generation, r)
	if ctx.Err() != nil {
		return model.OperationResult{}, ctx.Err()
	}
	if err != nil {
		return model.OperationResult{}, err
	}
	if reply.Error != nil {
		return model.OperationResult{}, reply.Error
	}
	return model.OperationResult{Output: reply.Output}, nil
}
