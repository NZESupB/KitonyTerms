package session

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"strconv"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
	"kitonyterms/internal/monitor"
)

func monitorFixtureBlock(sample int) string {
	cpu := fmt.Sprintf("cpu %d 0 0 %d\ncpu0 %d 0 0 %d\n", 100+50*sample, 100+150*sample, 100+50*sample, 100+150*sample)
	sections := []string{cpu, "MemTotal: 1024 kB\nMemAvailable: 512 kB\n", fmt.Sprintf("eth0: %d 0 0 0 0 0 0 0 %d 0 0 0 0 0 0 0\n", 100+200*sample, 200+300*sample), "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 1000 400 600 40% /\n", "0.25\n", "3600.5\n", "hostname=fixture\narch=x86_64\n"}
	return monitor.Begin + "\n" + strings.Join(sections, monitor.Section+"\n") + monitor.End + "\n"
}
func waitMonitor(t *testing.T, m *Manager, s model.SessionSnapshot, watch string, match func(model.MonitorSnapshot) bool) model.MonitorSnapshot {
	t.Helper()
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		value, err := m.PollMonitor(s.ID, s.Generation, watch)
		if err != nil {
			t.Fatal(err)
		}
		if match(value) {
			return value
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("监控快照未收敛")
	return model.MonitorSnapshot{}
}
func waitMonitorDone(t *testing.T, run *monitorRun) {
	t.Helper()
	select {
	case <-run.done:
	case <-time.After(2 * time.Second):
		t.Fatal("监控通道未回收")
	}
}

func TestMonitorSSHSharedOwnershipFailureRetentionAndLease(t *testing.T) {
	var opened, active atomic.Int32
	server := startLoopbackHooks(t, nil, func(ch ssh.Channel, req *ssh.Request) bool {
		var request struct{ Command string }
		if ssh.Unmarshal(req.Payload, &request) != nil || request.Command != "sh" {
			return false
		}
		opened.Add(1)
		active.Add(1)
		defer active.Add(-1)
		_ = req.Reply(true, nil)
		reader := bufio.NewReader(ch)
		sample := 0
		for {
			line, err := reader.ReadString('\n')
			if err != nil {
				return true
			}
			if strings.HasPrefix(line, "printf '__KTM_HEARTBEAT__") {
				marker := strings.TrimSuffix(strings.TrimPrefix(line, "printf '"), "\\n'\n")
				_, _ = io.WriteString(ch, marker+"\n")
			} else if line == monitor.Command {
				if sample == 0 {
					time.Sleep(300 * time.Millisecond)
				}
				if sample == 2 {
					return true
				}
				_, _ = io.WriteString(ch, monitorFixtureBlock(sample))
				sample++
			} else {
				t.Error("监控通道接收了非固定命令")
				return true
			}
		}
	})
	m, _ := newLoopbackManager(t, server, "tester")
	openedSession, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	s := waitConnected(t, m, openedSession, true)
	watchA, watchB := randomID(), randomID()
	first := waitMonitor(t, m, s, watchA, func(v model.MonitorSnapshot) bool { return v.Sequence == "1" })
	latency, err := strconv.Atoi(first.Stats.LatencyMS)
	if err != nil || latency >= 250 || latency < 1 || first.Stats.CPUPercent != nil {
		t.Fatal("采样耗时污染心跳/首样本", first, err)
	}
	if _, err = m.PollMonitor(s.ID, s.Generation, watchB); err != nil {
		t.Fatal(err)
	}
	if err = m.StopMonitor(s.ID, s.Generation, watchA); err != nil {
		t.Fatal(err)
	}
	second := waitMonitor(t, m, s, watchB, func(v model.MonitorSnapshot) bool { return v.Sequence == "2" })
	if *second.Stats.CPUPercent != 25 || second.Stats.NetRXRate == "" || opened.Load() != 1 {
		t.Fatal("未复用通道或差分错误", second)
	}
	*second.Stats.CPUPercent = 99
	copy, _ := m.PollMonitor(s.ID, s.Generation, watchB)
	if *copy.Stats.CPUPercent != 25 {
		t.Fatal("快照可修改内部状态")
	}
	if _, err = m.PollMonitor(s.ID, "stale", watchB); err == nil {
		t.Fatal("旧代次读监控")
	}
	if err = m.StopMonitor(s.ID, "stale", watchB); err == nil {
		t.Fatal("旧代次停监控")
	}
	failed := waitMonitor(t, m, s, watchB, func(v model.MonitorSnapshot) bool { return v.Status == "error" })
	if failed.Sequence != "2" || failed.Stats == nil || *failed.Stats.CPUPercent != 25 || failed.Error == "" {
		t.Fatal("失败丢失成功快照")
	}
	if _, err = m.ReadDirectory(context.Background(), s.ID, s.Generation, server.root); err != nil {
		t.Fatal("监控关闭污染SFTP", err)
	}
	host, _ := m.Get(s.ID, s.Generation)
	host.mu.Lock()
	run := host.monitor
	host.mu.Unlock()
	_ = m.StopMonitor(s.ID, s.Generation, watchB)
	waitMonitorDone(t, run)
	next := waitMonitor(t, m, s, watchA, func(v model.MonitorSnapshot) bool { return v.Sequence == "3" })
	if next.Stats.CPUPercent != nil || opened.Load() != 2 {
		t.Fatal("重启沿用过期基线", next)
	}
	// 旧订阅的迟到清理不能停止新订阅。
	_ = m.StopMonitor(s.ID, s.Generation, watchB)
	host.mu.Lock()
	run = host.monitor
	if run.ctx.Err() != nil {
		t.Fatal("旧清理停止新订阅")
	}
	// 直接推进租约截止时间，验证无客户端续期时的回收路径。
	run.watchers[watchA] = time.Now().Add(-time.Second)
	host.mu.Unlock()
	waitMonitorDone(t, run)
	if active.Load() != 0 {
		t.Fatal("租约过期未关闭远端通道")
	}
	m.Shutdown()
}
func TestMonitorHeartbeatTimeoutAndLateSequence(t *testing.T) {
	server := startLoopbackHooks(t, nil, func(ch ssh.Channel, req *ssh.Request) bool {
		var request struct{ Command string }
		if ssh.Unmarshal(req.Payload, &request) != nil || request.Command != "sh" {
			return false
		}
		_ = req.Reply(true, nil)
		reader := bufio.NewReader(ch)
		sample := 0
		for {
			line, err := reader.ReadString('\n')
			if err != nil {
				return true
			}
			if line == monitor.Command {
				if sample == 0 {
					_, _ = io.WriteString(ch, "__KTM_HEARTBEAT__1__\n")
				}
				_, _ = io.WriteString(ch, monitorFixtureBlock(sample))
				sample++
			} else if strings.Contains(line, "__KTM_HEARTBEAT__2__") {
				_, _ = io.WriteString(ch, "__KTM_HEARTBEAT__1__\n")
				time.Sleep(40 * time.Millisecond)
				_, _ = io.WriteString(ch, "__KTM_HEARTBEAT__2__\n")
			}
		}
	})
	m, _ := newLoopbackManager(t, server, "tester")
	opened, _ := m.Connect("loopback")
	s := waitConnected(t, m, opened, true)
	watch := randomID()
	first := waitMonitor(t, m, s, watch, func(v model.MonitorSnapshot) bool { return v.Sequence == "1" })
	if first.Stats.LatencyMS != "" {
		t.Fatal("心跳超时不是未知")
	}
	second := waitMonitor(t, m, s, watch, func(v model.MonitorSnapshot) bool { return v.Sequence == "2" })
	latency, _ := strconv.Atoi(second.Stats.LatencyMS)
	if latency < 35 || latency > 1000 {
		t.Fatal("迟到标记满足新心跳", latency)
	}
	if err := m.CloseSession(s.ID, s.Generation); err != nil {
		t.Fatal(err)
	}
	m.Shutdown()
}
func TestMonitorFramingBoundsAndCancellation(t *testing.T) {
	lines := make(chan monitorLine, 4)
	lines <- monitorLine{text: "__KTM_HEARTBEAT__1__"}
	lines <- monitorLine{text: "prefix __KTM_HEARTBEAT__2__"}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Millisecond)
	defer cancel()
	if _, err := (monitorStream{lines}).readUntil(ctx, "__KTM_HEARTBEAT__2__", false); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal("旧/非整行标记被接受", err)
	}
	lines2 := make(chan monitorLine, 20)
	for i := 0; i < 17; i++ {
		lines2 <- monitorLine{text: strings.Repeat("x", 64<<10)}
	}
	close(lines2)
	if _, err := (monitorStream{lines2}).readUntil(context.Background(), monitor.End, true); err == nil || !strings.Contains(err.Error(), "1 MiB") {
		t.Fatal("未限制帧总量", err)
	}
	started := make(chan struct{}, 1)
	server := startLoopbackHooks(t, nil, func(ch ssh.Channel, req *ssh.Request) bool {
		var r struct{ Command string }
		if ssh.Unmarshal(req.Payload, &r) != nil || r.Command != "sh" {
			return false
		}
		_ = req.Reply(true, nil)
		started <- struct{}{}
		_, _ = io.Copy(io.Discard, ch)
		return true
	})
	m, _ := newLoopbackManager(t, server, "tester")
	opened, _ := m.Connect("loopback")
	s := waitConnected(t, m, opened, true)
	watch := randomID()
	_, _ = m.PollMonitor(s.ID, s.Generation, watch)
	<-started
	host, _ := m.Get(s.ID, s.Generation)
	host.mu.Lock()
	run := host.monitor
	host.mu.Unlock()
	_ = m.StopMonitor(s.ID, s.Generation, watch)
	waitMonitorDone(t, run)
	if _, err := m.ReadDirectory(context.Background(), s.ID, s.Generation, server.root); err != nil {
		t.Fatal("取消采样关闭宿主", err)
	}
	m.Shutdown()
}

func TestMonitorSSHOutputLimitsAndExecNegotiationCancellation(t *testing.T) {
	for _, mode := range []string{"line", "stderr", "negotiation"} {
		t.Run(mode, func(t *testing.T) {
			started, release := make(chan struct{}), make(chan struct{})
			server := startLoopbackHooks(t, nil, func(ch ssh.Channel, req *ssh.Request) bool {
				var request struct{ Command string }
				if ssh.Unmarshal(req.Payload, &request) != nil || request.Command != "sh" {
					return false
				}
				close(started)
				if mode == "negotiation" {
					<-release
					return true
				}
				_ = req.Reply(true, nil)
				if mode == "line" {
					_, _ = io.WriteString(ch, strings.Repeat("x", monitor.MaxLine+1))
				} else {
					_, _ = io.WriteString(ch.Stderr(), strings.Repeat("synthetic-private-marker", 4000))
				}
				_, _ = io.Copy(io.Discard, ch)
				return true
			})
			defer close(release)
			m, _ := newLoopbackManager(t, server, "tester")
			opened, _ := m.Connect("loopback")
			s := waitConnected(t, m, opened, true)
			watch := randomID()
			_, _ = m.PollMonitor(s.ID, s.Generation, watch)
			<-started
			host, _ := m.Get(s.ID, s.Generation)
			host.mu.Lock()
			run := host.monitor
			host.mu.Unlock()
			if mode != "negotiation" {
				value := waitMonitor(t, m, s, watch, func(v model.MonitorSnapshot) bool { return v.Status == "error" })
				if value.Stats != nil || strings.Contains(value.Error, "synthetic-private-marker") {
					t.Fatal("超限载荷进入快照")
				}
			}
			_ = m.StopMonitor(s.ID, s.Generation, watch)
			waitMonitorDone(t, run)
			if _, err := m.ReadDirectory(context.Background(), s.ID, s.Generation, server.root); err != nil {
				t.Fatal("监控回收污染SFTP", err)
			}
			m.Shutdown()
		})
	}
}

func TestMonitorInvalidSubscription(t *testing.T) {
	m := &Manager{sessions: map[string]*Session{}}
	for _, id := range []string{"", "short", strings.Repeat("z", 32), strings.Repeat("a", 1<<20)} {
		if _, err := m.PollMonitor("missing", "missing", id); err == nil || err.Error() != "监控订阅标识无效" {
			t.Fatal("无效订阅未在查询会话前拒绝", err)
		}
	}
}
