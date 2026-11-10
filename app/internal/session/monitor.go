package session

import (
	"bufio"
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"strconv"
	"strings"
	"sync"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
	"kitonyterms/internal/monitor"
)

const monitorLease = 5 * time.Second

type monitorRun struct {
	ctx      context.Context
	cancel   context.CancelFunc
	done     chan struct{}
	watchers map[string]time.Time
	snapshot model.MonitorSnapshot
	sequence uint64
}

// 查询续期而非启动一次性 exec；同一宿主最多一条采集通道。
func (m *Manager) PollMonitor(id, generation, watchID string) (model.MonitorSnapshot, error) {
	if len(watchID) != 32 {
		return model.MonitorSnapshot{}, fmt.Errorf("监控订阅标识无效")
	}
	if value, err := hex.DecodeString(watchID); err != nil || len(value) != 16 {
		return model.MonitorSnapshot{}, fmt.Errorf("监控订阅标识无效")
	}
	s, err := m.Get(id, generation)
	if err != nil {
		return model.MonitorSnapshot{}, err
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.closed || m.draining != "" {
		return model.MonitorSnapshot{}, fmt.Errorf("应用正在关闭")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.client == nil || s.status != "connected" || s.ctx.Err() != nil {
		return model.MonitorSnapshot{}, fmt.Errorf("SSH 会话未连接")
	}
	run := s.monitor
	if run == nil || monitorDone(run) {
		ctx, cancel := context.WithCancel(s.ctx)
		next := &monitorRun{ctx: ctx, cancel: cancel, done: make(chan struct{}), watchers: map[string]time.Time{}, snapshot: model.MonitorSnapshot{SessionID: id, Generation: generation, Status: "loading", Sequence: "0"}}
		if run != nil {
			next.snapshot.Stats, next.snapshot.SampledAt, next.sequence = run.snapshot.Stats, run.snapshot.SampledAt, run.sequence
			next.snapshot.Sequence = strconv.FormatUint(next.sequence, 10)
		}
		run = next
		s.monitor = run
		m.addWorkerLocked()
		go func() { defer m.workerDone(); defer close(run.done); m.monitorLoop(s, run) }()
	}
	for key, until := range run.watchers {
		if time.Now().After(until) {
			delete(run.watchers, key)
		}
	}
	if _, exists := run.watchers[watchID]; !exists && len(run.watchers) >= 8 {
		return model.MonitorSnapshot{}, fmt.Errorf("监控订阅过多")
	}
	run.watchers[watchID] = time.Now().Add(monitorLease)
	return cloneMonitorSnapshot(run.snapshot), nil
}
func monitorDone(run *monitorRun) bool {
	select {
	case <-run.done:
		return true
	default:
		return false
	}
}
func cloneMonitorSnapshot(snapshot model.MonitorSnapshot) model.MonitorSnapshot {
	if snapshot.Stats != nil {
		stats := *snapshot.Stats
		stats.PerCore = append([]model.MonitorCore{}, stats.PerCore...)
		stats.Interfaces = append([]model.MonitorInterface{}, stats.Interfaces...)
		stats.Disks = append([]model.MonitorDisk{}, stats.Disks...)
		clone := func(value *float64) *float64 {
			if value == nil {
				return nil
			}
			copy := *value
			return &copy
		}
		stats.CPUPercent, stats.Load1 = clone(stats.CPUPercent), clone(stats.Load1)
		for i := range stats.PerCore {
			stats.PerCore[i].Percent = clone(stats.PerCore[i].Percent)
		}
		snapshot.Stats = &stats
	}
	return snapshot
}
func (m *Manager) StopMonitor(id, generation, watchID string) error {
	s, err := m.Get(id, generation)
	if err != nil {
		return err
	}
	s.mu.Lock()
	run := s.monitor
	if run != nil {
		delete(run.watchers, watchID)
		if len(run.watchers) == 0 {
			run.cancel()
		}
	}
	s.mu.Unlock()
	return nil
}
func (m *Manager) monitorLoop(s *Session, run *monitorRun) {
	watchDone := make(chan struct{})
	go func() {
		defer close(watchDone)
		ticker := time.NewTicker(250 * time.Millisecond)
		defer ticker.Stop()
		for {
			select {
			case <-run.ctx.Done():
				return
			case now := <-ticker.C:
				s.mu.Lock()
				for key, until := range run.watchers {
					if !now.Before(until) {
						delete(run.watchers, key)
					}
				}
				if len(run.watchers) == 0 {
					run.cancel()
				}
				s.mu.Unlock()
			}
		}
	}()
	defer func() {
		run.cancel()
		<-watchDone
		s.mu.Lock()
		run.snapshot.Status = "paused"
		if s.ctx.Err() != nil {
			run.snapshot.Status = "disconnected"
		}
		s.mu.Unlock()
	}()
	backoff := time.Second
	for run.ctx.Err() == nil {
		s.mu.Lock()
		host := s.client
		s.mu.Unlock()
		if host == nil {
			return
		}
		err := collectMonitor(run.ctx, host, func(stats *model.MonitorStats) {
			s.mu.Lock()
			defer s.mu.Unlock()
			if run.ctx.Err() != nil || s.monitor != run {
				return
			}
			run.sequence++
			run.snapshot = model.MonitorSnapshot{SessionID: s.id, Generation: s.generation, Sequence: strconv.FormatUint(run.sequence, 10), Status: "running", SampledAt: time.Now().UTC().Format(time.RFC3339Nano), Stats: stats}
			backoff = time.Second
		})
		if run.ctx.Err() != nil {
			return
		}
		s.mu.Lock()
		run.snapshot.Status, run.snapshot.Error = "error", err.Error()
		s.mu.Unlock()
		backoff = min(backoff*2, 30*time.Second)
		timer := time.NewTimer(backoff)
		select {
		case <-run.ctx.Done():
			timer.Stop()
			return
		case <-timer.C:
		}
	}
}

type monitorLine struct {
	text string
	err  error
}
type monitorStream struct{ lines <-chan monitorLine }

// 按完整行匹配哨兵，旧心跳和包含哨兵的普通文本不能完成新请求。
func (stream monitorStream) readUntil(ctx context.Context, marker string, capture bool) (string, error) {
	var output strings.Builder
	size := 0
	started := !capture
	for {
		select {
		case <-ctx.Done():
			return "", ctx.Err()
		case line, ok := <-stream.lines:
			if !ok {
				return "", io.EOF
			}
			if line.err != nil {
				return "", line.err
			}
			size += len(line.text) + 1
			if size > monitor.MaxOutput {
				return "", fmt.Errorf("资源监控输出超过 1 MiB")
			}
			if capture && line.text == monitor.Begin {
				if started {
					return "", fmt.Errorf("资源监控采样边界重复")
				}
				started = true
			}
			if started && capture {
				output.WriteString(line.text)
				output.WriteByte('\n')
			}
			if line.text == marker && started {
				return output.String(), nil
			}
		}
	}
}
func collectMonitor(ctx context.Context, host *ssh.Client, publish func(*model.MonitorStats)) error {
	type opened struct {
		ch       ssh.Channel
		requests <-chan *ssh.Request
	}
	openCtx, cancelOpen := context.WithTimeout(ctx, 4*time.Second)
	defer cancelOpen()
	value, err := negotiation(openCtx, host, nil, func() (opened, error) {
		ch, requests, err := host.OpenChannel("session", nil)
		return opened{ch, requests}, err
	})
	if err != nil {
		if value.ch != nil {
			closeOperationChannel(host, value.ch)
		}
		return fmt.Errorf("无法打开 SSH 监控通道")
	}
	ch := value.ch
	ctx, cancel := context.WithCancel(ctx)
	var streams sync.WaitGroup
	streams.Add(3)
	lines := make(chan monitorLine, 8)
	defer func() {
		cancel()
		closeOperationChannel(host, ch)
		drained := make(chan struct{})
		go func() { streams.Wait(); close(drained) }()
		timer := time.AfterFunc(500*time.Millisecond, func() { _ = host.Close() })
		<-drained
		timer.Stop()
	}()
	go func() {
		defer streams.Done()
		defer close(lines)
		scanner := bufio.NewScanner(ch)
		scanner.Buffer(make([]byte, 4096), monitor.MaxLine)
		for scanner.Scan() {
			select {
			case lines <- monitorLine{text: scanner.Text()}:
			case <-ctx.Done():
				return
			}
		}
		if scanner.Err() != nil {
			select {
			case lines <- monitorLine{err: fmt.Errorf("资源监控单行输出无效或超过 64 KiB")}:
			case <-ctx.Done():
			}
		}
	}()
	go func() {
		defer streams.Done()
		// stderr 不进入快照或日志，避免远端错误回显载荷；总量仍有界。
		if n, _ := io.CopyN(io.Discard, ch.Stderr(), (64<<10)+1); n > 64<<10 {
			cancel()
		}
	}()
	go func() { defer streams.Done(); ssh.DiscardRequests(value.requests) }()
	_, err = negotiation(openCtx, host, ch, func() (bool, error) {
		ok, err := ch.SendRequest("exec", true, ssh.Marshal(struct{ Command string }{"sh"}))
		if err == nil && !ok {
			err = fmt.Errorf("远端拒绝监控 shell")
		}
		return ok, err
	})
	if err != nil {
		return fmt.Errorf("远端拒绝或超时启动监控 shell")
	}
	cancelOpen()
	stream := monitorStream{lines: lines}
	parser := monitor.Parser{}
	sequence := uint64(0)
	for {
		sequence++
		command, marker := monitor.Heartbeat(sequence)
		started := time.Now()
		if err := monitorWrite(ctx, host, ch, command); err != nil {
			return err
		}
		heartbeatCtx, cancelHeartbeat := context.WithTimeout(ctx, 4*time.Second)
		_, err = stream.readUntil(heartbeatCtx, marker, false)
		latency := ""
		if err == nil {
			latency = strconv.FormatInt(max(1, time.Since(started).Milliseconds()), 10)
		}
		cancelHeartbeat()
		if err != nil && !errors.Is(err, context.DeadlineExceeded) {
			return fmt.Errorf("SSH 监控心跳读取失败")
		}
		if err = monitorWrite(ctx, host, ch, monitor.Command); err != nil {
			return err
		}
		sampleCtx, cancelSample := context.WithTimeout(ctx, 12*time.Second)
		raw, err := stream.readUntil(sampleCtx, monitor.End, true)
		cancelSample()
		if err != nil {
			if errors.Is(err, context.DeadlineExceeded) {
				return fmt.Errorf("资源监控采样超时（12 秒）")
			}
			return fmt.Errorf("资源监控输出中断或超过限制")
		}
		stats, err := parser.Parse(raw, time.Now())
		if err != nil {
			return err
		}
		stats.LatencyMS = latency
		publish(stats)
		timer := time.NewTimer(2 * time.Second)
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-timer.C:
		}
	}
}
func monitorWrite(ctx context.Context, host *ssh.Client, ch ssh.Channel, text string) error {
	ctx, cancel := context.WithTimeout(ctx, 4*time.Second)
	defer cancel()
	_, err := negotiation(ctx, host, ch, func() (int, error) {
		n, err := io.WriteString(ch, text)
		if err == nil && n != len(text) {
			err = io.ErrShortWrite
		}
		return n, err
	})
	if err != nil {
		return fmt.Errorf("SSH 监控命令发送失败")
	}
	return nil
}
