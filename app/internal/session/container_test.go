package session

import (
	"bufio"
	"bytes"
	"context"
	"encoding/base64"
	"io"
	"strings"
	"testing"
	"time"

	"golang.org/x/crypto/ssh"
	"kitonyterms/internal/model"
	"kitonyterms/internal/remoteops"
)

func waitContainer(t *testing.T, m *Manager, s model.SessionSnapshot, id, status string) model.ContainerTerminalSnapshot {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		value, err := m.GetContainerTerminal(s.ID, s.Generation, id)
		if err != nil {
			t.Fatal(err)
		}
		if value.Status == status {
			return value
		}
		if value.Status == "closed" {
			t.Fatalf("容器终端提前关闭: %s", value.Error)
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatalf("容器终端未进入 %s", status)
	return model.ContainerTerminalSnapshot{}
}
func containerOutput(t *testing.T, m *Manager, s model.SessionSnapshot, id, wanted string) string {
	t.Helper()
	var result strings.Builder
	after := "0"
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		batch, err := m.PollContainerTerminal(s.ID, s.Generation, id, after)
		if err != nil {
			t.Fatal(err)
		}
		for _, chunk := range batch.Chunks {
			data, err := base64.StdEncoding.DecodeString(chunk.Data)
			if err != nil {
				t.Fatal(err)
			}
			result.Write(data)
			after = chunk.Sequence
		}
		if strings.Contains(result.String(), wanted) {
			return result.String()
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("未收到容器字节输出")
	return ""
}
func TestContainerTerminalIsolationReplayAndLifecycle(t *testing.T) {
	sizes := make(chan [2]uint32, 8)
	server := startLoopbackHooks(t, nil, nil, func(ch ssh.Channel, req *ssh.Request) bool {
		if req.Type == "window-change" {
			var size struct{ Cols, Rows, Width, Height uint32 }
			if ssh.Unmarshal(req.Payload, &size) == nil {
				sizes <- [2]uint32{size.Cols, size.Rows}
			}
			return true
		}
		if req.Type != "exec" {
			return false
		}
		var command struct{ Command string }
		if ssh.Unmarshal(req.Payload, &command) != nil || command.Command != "docker exec -it 'container-1' /bin/sh" {
			return false
		}
		_ = req.Reply(true, nil)
		go func() {
			defer ch.Close()
			_, _ = ch.Write([]byte("container-ready\r\n"))
			reader := bufio.NewReader(ch)
			for {
				data, err := reader.ReadBytes('\n')
				if bytes.Equal(data, []byte("exit\n")) {
					_ = ch.CloseWrite()
					time.Sleep(20 * time.Millisecond)
					_, _ = ch.SendRequest("exit-status", false, ssh.Marshal(struct{ Status uint32 }{42}))
					return
				}
				if len(data) > 0 {
					_, _ = ch.Write(data)
				}
				if err != nil {
					return
				}
			}
		}()
		return true
	})
	m, _ := newLoopbackManager(t, server, "tester")
	opened, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	s := waitConnected(t, m, opened, true)
	if current, err := m.GetContainerTerminal(s.ID, s.Generation, ""); err != nil || current.ExecID != "" {
		t.Fatalf("无终端时发现异常: %+v %v", current, err)
	}
	r := model.ContainerTerminalRequest{ExecID: "exec-1", Container: "container-1", Cols: 90, Rows: 32}
	if _, err = m.StartContainerTerminal(s.ID, s.Generation, r); err != nil {
		t.Fatal(err)
	}
	waitContainer(t, m, s, r.ExecID, "running")
	if activity := m.Activity(); activity.Sessions != 1 || activity.ContainerTerminals != 1 {
		t.Fatalf("容器活动未统计: %+v", activity)
	}
	if current, err := m.GetContainerTerminal(s.ID, s.Generation, ""); err != nil || current.ExecID != r.ExecID || current.Container != r.Container {
		t.Fatalf("未恢复当前终端: %+v %v", current, err)
	}
	if _, err := m.GetContainerTerminal(s.ID, "stale", ""); err == nil {
		t.Fatal("发现接口接受旧代次")
	}
	if _, err := m.PollContainerTerminal(s.ID, s.Generation, "", "0"); err == nil {
		t.Fatal("输出接口接受空 ExecID")
	}
	if _, err = m.StartContainerTerminal(s.ID, s.Generation, r); err != nil {
		t.Fatal("启动重试未去重", err)
	}
	other := r
	other.ExecID = "exec-2"
	if _, err = m.StartContainerTerminal(s.ID, s.Generation, other); err == nil {
		t.Fatal("允许并发容器终端")
	}
	if err = m.ResizeContainerTerminal(context.Background(), s.ID, s.Generation, r.ExecID, 111, 37); err != nil {
		t.Fatal(err)
	}
	select {
	case size := <-sizes:
		if size != [2]uint32{111, 37} {
			t.Fatal("PTY 尺寸不匹配")
		}
	case <-time.After(time.Second):
		t.Fatal("PTY 调整未发送")
	}
	data := []byte("child-中文\xff\n")
	input := model.ContainerInputRequest{SessionID: s.ID, Generation: s.Generation, ExecID: r.ExecID, Sequence: "1", Data: base64.StdEncoding.EncodeToString(data)}
	if err = m.SendContainerInput(context.Background(), input); err != nil {
		t.Fatal(err)
	}
	if err = m.SendContainerInput(context.Background(), input); err != nil {
		t.Fatal("输入重试失败", err)
	}
	input.Data = base64.StdEncoding.EncodeToString([]byte("other\n"))
	if m.SendContainerInput(context.Background(), input) == nil {
		t.Fatal("接受同序不同输入")
	}
	output := containerOutput(t, m, s, r.ExecID, string(data))
	if strings.Count(output, string(data)) != 1 {
		t.Fatal("输入重复或字节损坏")
	}
	replayed := containerOutput(t, m, s, r.ExecID, string(data))
	if replayed != output {
		t.Fatal("重建输出不一致")
	}
	if state := waitContainer(t, m, s, r.ExecID, "running"); state.InputSequence != "1" {
		t.Fatal("输入序号未恢复")
	}
	host, err := m.Poll(s.ID, s.Generation, "0")
	if err != nil {
		t.Fatal(err)
	}
	for _, chunk := range host.Chunks {
		decoded, _ := base64.StdEncoding.DecodeString(chunk.Data)
		if bytes.Contains(decoded, []byte("child-")) {
			t.Fatal("容器输出污染宿主")
		}
	}
	if _, err = m.GetContainerTerminal(s.ID, "stale", r.ExecID); err == nil {
		t.Fatal("接受旧连接代次")
	}
	if _, err = m.PollContainerTerminal(s.ID, s.Generation, "stale", "0"); err == nil {
		t.Fatal("接受旧 ExecID")
	}
	input.Sequence = "2"
	input.Data = base64.StdEncoding.EncodeToString([]byte("exit\n"))
	if err = m.SendContainerInput(context.Background(), input); err != nil {
		t.Fatal(err)
	}
	closed := waitContainer(t, m, s, r.ExecID, "closed")
	if !strings.Contains(closed.Error, "42") {
		t.Fatal("EOF 后退出状态丢失")
	}
	if current, err := m.GetContainerTerminal(s.ID, s.Generation, ""); err != nil || current.ExecID != r.ExecID || current.Status != "closed" {
		t.Fatalf("自然结束的终端历史无法恢复: %+v %v", current, err)
	}
	other.Container = "container-1"
	if _, err = m.StartContainerTerminal(s.ID, s.Generation, other); err != nil {
		t.Fatal(err)
	}
	waitContainer(t, m, s, other.ExecID, "running")
	if err = m.CloseContainerTerminal(s.ID, s.Generation, other.ExecID); err != nil {
		t.Fatal(err)
	}
	if closed = waitContainer(t, m, s, other.ExecID, "closed"); closed.Error != "" {
		t.Fatal("主动关闭产生错误", closed.Error)
	}
	if activity := m.Activity(); activity.Sessions != 1 || activity.ContainerTerminals != 0 {
		t.Fatalf("已关闭容器仍计入活动: %+v", activity)
	}
	if current, err := m.GetContainerTerminal(s.ID, s.Generation, ""); err != nil || current.ExecID != "" {
		t.Fatalf("显式关闭后仍发现终端: %+v %v", current, err)
	}
	if err := m.CloseContainerTerminal(s.ID, s.Generation, other.ExecID); err != nil {
		t.Fatal("重复关闭未保持幂等", err)
	}
	if _, err = m.ReadDirectory(context.Background(), s.ID, s.Generation, server.root); err != nil {
		t.Fatal("容器关闭破坏 SFTP", err)
	}
	if err = m.Input(model.InputRequest{SessionID: s.ID, Generation: s.Generation, Sequence: "1", Data: base64.StdEncoding.EncodeToString([]byte("host-only\n"))}); err != nil {
		t.Fatal("宿主输入失效", err)
	}
}
func TestContainerTerminalCancelNegotiationBackpressureAndDisconnect(t *testing.T) {
	started := make(chan struct{}, 4)
	server := startLoopbackHooks(t, nil, nil, func(ch ssh.Channel, req *ssh.Request) bool {
		if req.Type != "exec" {
			return false
		}
		var command struct{ Command string }
		_ = ssh.Unmarshal(req.Payload, &command)
		if !strings.HasPrefix(command.Command, "docker exec") {
			return false
		}
		started <- struct{}{}
		if strings.Contains(command.Command, "'wait'") {
			_, _ = io.Copy(io.Discard, ch)
			return true
		}
		_ = req.Reply(true, nil)
		go func() {
			defer ch.Close()
			_, _ = ch.Write(bytes.Repeat([]byte("x"), 2<<20))
			_, _ = io.Copy(io.Discard, ch)
		}()
		return true
	})
	m, _ := newLoopbackManager(t, server, "tester")
	opened, err := m.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	s := waitConnected(t, m, opened, true)
	for _, target := range []string{"wait", "flood"} {
		r := model.ContainerTerminalRequest{ExecID: target, Container: target, Cols: 80, Rows: 24}
		if _, err = m.StartContainerTerminal(s.ID, s.Generation, r); err != nil {
			t.Fatal(err)
		}
		select {
		case <-started:
		case <-time.After(time.Second):
			t.Fatal("容器未开始")
		}
		if target == "flood" {
			c, _ := m.containerTerminal(s.ID, s.Generation, r.ExecID)
			deadline := time.Now().Add(time.Second)
			for time.Now().Before(deadline) {
				c.output.mu.Lock()
				size := c.output.bytes
				c.output.mu.Unlock()
				if size == outputBudget {
					break
				}
				time.Sleep(5 * time.Millisecond)
			}
			c.output.mu.Lock()
			size := c.output.bytes
			c.output.mu.Unlock()
			if size != outputBudget {
				t.Fatalf("输出预算异常: %d", size)
			}
		}
		begin := time.Now()
		if err = m.CloseContainerTerminal(s.ID, s.Generation, r.ExecID); err != nil {
			t.Fatal(err)
		}
		if time.Since(begin) > 1500*time.Millisecond {
			t.Fatal("关闭未收敛")
		}
		waitContainer(t, m, s, r.ExecID, "closed")
		if _, err = m.ReadDirectory(context.Background(), s.ID, s.Generation, server.root); err != nil {
			t.Fatal("取消影响宿主", err)
		}
	}
	r := model.ContainerTerminalRequest{ExecID: "disconnect", Container: "flood", Cols: 80, Rows: 24}
	if _, err = m.StartContainerTerminal(s.ID, s.Generation, r); err != nil {
		t.Fatal(err)
	}
	c, _ := m.containerTerminal(s.ID, s.Generation, r.ExecID)
	if err = m.CloseSession(s.ID, s.Generation); err != nil {
		t.Fatal(err)
	}
	select {
	case <-c.done:
	case <-time.After(2 * time.Second):
		t.Fatal("宿主断线未回收容器")
	}
}
func TestContainerTerminalRequestValidation(t *testing.T) {
	for _, target := range []string{"", "-flag", "name;id", "name\n", "name'", "$(id)", strings.Repeat("x", 129)} {
		if _, err := remoteops.ContainerCommand("exec", target); err == nil {
			t.Fatalf("接受无效容器标识 %q", target)
		}
	}
	if _, err := remoteops.ContainerCommand("bad id", "valid"); err == nil {
		t.Fatal("接受无效 ExecID")
	}
}
