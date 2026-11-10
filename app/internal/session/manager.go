package session

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"io"
	"sort"
	"strconv"
	"sync"
	"time"

	"github.com/pkg/sftp"
	"golang.org/x/crypto/ssh"
	"golang.org/x/crypto/ssh/agent"
	"kitonyterms/internal/model"
	"kitonyterms/internal/shellintegration"
	"kitonyterms/internal/storage"
)

type Manager struct {
	mu       sync.Mutex
	ctx      context.Context
	cancel   context.CancelFunc
	sessions map[string]*Session
	repo     *storage.Repository
	vault    *storage.Vault
	notify   func()
	wg       sync.WaitGroup
	workers  int
	idle     chan struct{}
	draining string
	closed   bool
	// 构造时确定，连接开始后保持不变；认证和转发共用同一连接策略。
	agentDial func(context.Context) (io.ReadWriteCloser, error)
}
type Session struct {
	mu            sync.Mutex
	inputMu       sync.Mutex
	fileMu        fileMutex
	id            string
	generation    string
	profile       model.Profile
	status        string
	failure       string
	ctx           context.Context
	cancel        context.CancelFunc
	done          chan struct{}
	client        *ssh.Client
	pty           *ssh.Session
	stdin         io.WriteCloser
	sftp          *sftp.Client
	challenge     *model.Challenge
	answer        chan model.ChallengeAnswer
	inputSequence uint64
	inputDigest   string
	output        *outputBuffer
	cleanup       []io.Closer
	transfers     []*transferJob
	operations    map[string]*operationRun
	capabilities  *model.OperationsCapabilities
	container     *containerTerminal
	shell         *shellintegration.Observer
	shellIO       *shellIO
	monitor       *monitorRun
}

func randomID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b[:])
}
func New(ctx context.Context, repo *storage.Repository, vault *storage.Vault, notify func()) *Manager {
	ctx, cancel := context.WithCancel(ctx)
	return &Manager{ctx: ctx, cancel: cancel, sessions: map[string]*Session{}, repo: repo, vault: vault, notify: notify, agentDial: openAgent}
}
func (s *Session) snapshot() model.SessionSnapshot {
	s.mu.Lock()
	defer s.mu.Unlock()
	var c *model.Challenge
	if s.challenge != nil {
		copy := *s.challenge
		copy.Prompts = append([]model.Prompt{}, s.challenge.Prompts...)
		c = &copy
	}
	return model.SessionSnapshot{ID: s.id, Generation: s.generation, Title: s.profile.Name, Status: s.status, Error: s.failure, Challenge: c}
}
func (m *Manager) Snapshots() []model.SessionSnapshot {
	m.mu.Lock()
	all := make([]*Session, 0, len(m.sessions))
	for _, s := range m.sessions {
		all = append(all, s)
	}
	m.mu.Unlock()
	out := make([]model.SessionSnapshot, 0, len(all))
	for _, s := range all {
		out = append(out, s.snapshot())
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}
func (m *Manager) Get(id, generation string) (*Session, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	s := m.sessions[id]
	if m.closed || m.draining != "" {
		return nil, fmt.Errorf("连接管理器正在收尾")
	}
	if s == nil || s.generation != generation {
		return nil, fmt.Errorf("会话或连接代次已失效")
	}
	return s, nil
}
func (m *Manager) Connect(name string) (model.SessionSnapshot, error) {
	p, err := m.repo.Profile(name)
	if err != nil {
		return model.SessionSnapshot{}, err
	}
	ctx, cancel := context.WithCancel(m.ctx)
	s := &Session{id: randomID(), generation: randomID(), profile: p, status: "connecting", ctx: ctx, cancel: cancel, done: make(chan struct{}), output: newOutput(), shell: shellintegration.NewObserver()}
	m.mu.Lock()
	if m.closed || m.draining != "" || m.ctx.Err() != nil {
		m.mu.Unlock()
		cancel()
		return model.SessionSnapshot{}, fmt.Errorf("应用正在关闭")
	}
	if len(m.sessions) >= 32 {
		m.mu.Unlock()
		cancel()
		return model.SessionSnapshot{}, fmt.Errorf("最多保留 32 个会话，请关闭不用的连接")
	}
	m.sessions[s.id] = s
	m.addWorkerLocked()
	m.mu.Unlock()
	go func() {
		defer m.workerDone()
		defer close(s.done)
		err := m.run(s)
		s.cancel()
		s.mu.Lock()
		if err != nil && s.status != "closing" {
			s.failure = err.Error()
		}
		s.status = "closed"
		s.challenge = nil
		failure := s.failure
		s.mu.Unlock()
		s.output.close(failure)
		m.notify()
	}()
	m.notify()
	return s.snapshot(), nil
}
func (m *Manager) CloseSession(id, generation string) error {
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	m.mu.Lock()
	delete(m.sessions, id)
	m.mu.Unlock()
	s.mu.Lock()
	s.status = "closing"
	s.mu.Unlock()
	s.cancel()
	m.notify()
	return nil
}
func (m *Manager) Shutdown() {
	m.mu.Lock()
	m.closed = true
	m.mu.Unlock()
	m.cancel()
	m.wg.Wait()
}

func (m *Manager) TerminalState(id, generation string) (model.TerminalState, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return model.TerminalState{}, err
	}
	s.inputMu.Lock()
	defer s.inputMu.Unlock()
	return model.TerminalState{InputSequence: strconv.FormatUint(s.inputSequence, 10)}, nil
}
func (m *Manager) ask(s *Session, c model.Challenge) (model.ChallengeAnswer, error) {
	c.ID = randomID()
	if c.Prompts == nil {
		c.Prompts = []model.Prompt{}
	}
	ch := make(chan model.ChallengeAnswer, 1)
	s.mu.Lock()
	s.challenge = &c
	s.answer = ch
	s.mu.Unlock()
	m.notify()
	defer func() {
		s.mu.Lock()
		if s.challenge != nil && s.challenge.ID == c.ID {
			s.challenge = nil
			s.answer = nil
		}
		s.mu.Unlock()
		m.notify()
	}()
	timer := time.NewTimer(45 * time.Second)
	defer timer.Stop()
	select {
	case <-s.ctx.Done():
		return model.ChallengeAnswer{}, s.ctx.Err()
	case <-timer.C:
		return model.ChallengeAnswer{}, fmt.Errorf("认证或主机密钥确认超时")
	case answer := <-ch:
		return answer, nil
	}
}
func (m *Manager) Answer(a model.ChallengeAnswer) error {
	s, err := m.Get(a.SessionID, a.Generation)
	if err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.challenge == nil || s.challenge.ID != a.ChallengeID || s.answer == nil {
		return fmt.Errorf("认证挑战已失效")
	}
	select {
	case s.answer <- a:
		s.answer = nil
		return nil
	default:
		return fmt.Errorf("认证答案已提交")
	}
}
func (m *Manager) Poll(id, generation, after string) (model.TerminalBatch, error) {
	s, err := m.Get(id, generation)
	if err != nil {
		return model.TerminalBatch{}, err
	}
	return s.output.poll(after)
}
func (m *Manager) Input(req model.InputRequest) error {
	s, err := m.Get(req.SessionID, req.Generation)
	if err != nil {
		return err
	}
	data, err := base64.StdEncoding.DecodeString(req.Data)
	if err != nil || len(data) > 64<<10 {
		return fmt.Errorf("终端输入编码或长度无效")
	}
	sequence, err := strconv.ParseUint(req.Sequence, 10, 64)
	if err != nil {
		return err
	}
	s.inputMu.Lock()
	defer s.inputMu.Unlock()
	s.mu.Lock()
	stdin := s.stdin
	status := s.status
	runtime := s.shellIO
	s.mu.Unlock()
	if stdin == nil || status != "connected" {
		return fmt.Errorf("终端尚未连接")
	}
	if sequence == s.inputSequence && sequence > 0 && s.inputDigest == digest(data) {
		return nil
	}
	if sequence != s.inputSequence+1 {
		return fmt.Errorf("终端输入序号不连续")
	}
	if runtime == nil {
		return fmt.Errorf("终端处理尚未就绪")
	}
	command, err := runtime.observe(data, "")
	if err != nil {
		return err
	}
	n, err := stdin.Write(data)
	if err != nil || n != len(data) {
		s.cancel()
		if err != nil {
			return err
		}
		return io.ErrShortWrite
	}
	s.shell.Commit(command, time.Now())
	s.inputSequence = sequence
	s.inputDigest = digest(data)
	return nil
}
func (m *Manager) Resize(id, generation string, cols, rows int) error {
	if cols < 2 || cols > 1000 || rows < 1 || rows > 500 {
		return fmt.Errorf("终端尺寸无效")
	}
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	s.mu.Lock()
	pty := s.pty
	s.mu.Unlock()
	if pty == nil {
		return nil
	}
	return pty.WindowChange(rows, cols)
}
func (m *Manager) run(s *Session) error {
	client, closers, err := m.dial(s)
	if err != nil {
		return err
	}
	defer func() {
		for i := len(closers) - 1; i >= 0; i-- {
			closers[i].Close()
		}
	}()
	stop := context.AfterFunc(s.ctx, func() { client.Close() })
	defer stop()
	pty, err := client.NewSession()
	if err != nil {
		return err
	}
	defer pty.Close()
	stdin, err := pty.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := pty.StdoutPipe()
	if err != nil {
		return err
	}
	stderr, err := pty.StderrPipe()
	if err != nil {
		return err
	}
	if err = pty.RequestPty("xterm-256color", 24, 80, ssh.TerminalModes{ssh.ECHO: 1, ssh.TTY_OP_ISPEED: 38400, ssh.TTY_OP_OSPEED: 38400}); err != nil {
		return err
	}
	if s.profile.ForwardAgent {
		agentConn, e := m.agentDial(s.ctx)
		if e != nil {
			return fmt.Errorf("连接转发 agent: %w", e)
		}
		defer agentConn.Close()
		if e = agent.ForwardToAgent(client, agent.NewClient(agentConn)); e != nil {
			return e
		}
		if e = agent.RequestAgentForwarding(pty); e != nil {
			return e
		}
	}
	if err = pty.Shell(); err != nil {
		return err
	}
	runtime := newShellIO(s.ctx, s.output, s.shell)
	defer func() { s.cancel(); <-runtime.done }()
	if n, writeErr := stdin.Write([]byte(shellintegration.Bootstrap)); writeErr != nil {
		return writeErr
	} else if n != len(shellintegration.Bootstrap) {
		return io.ErrShortWrite
	}
	s.mu.Lock()
	s.client = client
	s.pty = pty
	s.stdin = stdin
	s.shellIO = runtime
	s.status = "connected"
	s.mu.Unlock()
	m.notify()
	var streams sync.WaitGroup
	streams.Add(2)
	pump := func(reader io.Reader, stderr bool) {
		defer streams.Done()
		buffer := make([]byte, 32768)
		for {
			n, e := reader.Read(buffer)
			if n > 0 {
				if e := runtime.outputData(buffer[:n], stderr); e != nil {
					return
				}
			}
			if e != nil {
				return
			}
		}
	}
	go pump(stdout, false)
	go pump(stderr, true)
	err = pty.Wait()
	drain := time.AfterFunc(500*time.Millisecond, s.cancel)
	streams.Wait()
	s.inputMu.Lock()
	s.mu.Lock()
	s.stdin = nil
	s.client = nil
	s.pty = nil
	sf := s.sftp
	s.sftp = nil
	s.mu.Unlock()
	s.inputMu.Unlock()
	outputErr := runtime.finish()
	drained := drain.Stop()
	if sf != nil {
		sf.Close()
	}
	if !drained {
		return fmt.Errorf("终端关闭时输出未在 500ms 内消费，已保留缓冲区内容")
	}
	if err == nil {
		return outputErr
	}
	return err
}
