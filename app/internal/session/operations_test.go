package session

import (
	"bufio"
	"context"
	"golang.org/x/crypto/ssh"
	"io"
	"kitonyterms/internal/model"
	"kitonyterms/internal/remoteops"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

const operationCaps = `{"linux":true,"systemd":true,"ps":true,"proc":true,"ss":true,"proc_net":true,"docker_daemon":true,"compose":true}`

func TestOperationsTypedSSHAndSudoIsolation(t *testing.T) {
	var probes atomic.Int32
	credentials := make(chan bool, 1)
	server := startLoopbackHooks(t, nil, func(ch ssh.Channel, req *ssh.Request) bool {
		var request struct{ Command string }
		if ssh.Unmarshal(req.Payload, &request) != nil {
			return false
		}
		_ = req.Reply(true, nil)
		status := uint32(0)
		switch {
		case request.Command == remoteops.ProbeCommand:
			probes.Add(1)
			_, _ = io.WriteString(ch, operationCaps)
		case strings.Contains(request.Command, "list-units"):
			_, _ = io.WriteString(ch, "sshd.service loaded active running SSH server\n")
		case strings.Contains(request.Command, "sudo -S"):
			_, _ = io.WriteString(ch.Stderr(), "__KT_SUDO_")
			_, _ = io.WriteString(ch.Stderr(), "PROMPT__")
			password, err := bufio.NewReader(ch).ReadString('\n')
			credentials <- err == nil && password == "test-sudo-password\n"
			// 即使远端回显，动作结果也不能回传凭据。
			_, _ = io.WriteString(ch, password)
		case strings.Contains(request.Command, "failed.service"):
			_, _ = io.WriteString(ch.Stderr(), "Permission denied test-private-marker")
			status = 1
		default:
			_, _ = io.WriteString(ch, "Id=sshd.service\n")
		}
		_ = ch.CloseWrite()
		time.Sleep(20 * time.Millisecond)
		_, _ = ch.SendRequest("exit-status", false, ssh.Marshal(struct{ Status uint32 }{status}))
		return true
	})
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	snapshot := waitConnected(t, manager, opened, true)
	execute := func(r model.OperationRequest) model.OperationReply {
		t.Helper()
		reply, err := manager.ExecuteOperation(context.Background(), snapshot.ID, snapshot.Generation, r)
		if err != nil {
			t.Fatal(err)
		}
		return reply
	}
	reply := execute(model.OperationRequest{ID: "list", Kind: "services"})
	if reply.Error != nil || len(reply.Services) != 1 || reply.Services[0].Name != "sshd.service" {
		t.Fatal("类型化服务查询失败")
	}
	reply = execute(model.OperationRequest{ID: "details", Kind: "service_details", Target: "sshd.service"})
	if reply.Error != nil || !strings.Contains(reply.Output, "Id=sshd.service") || probes.Load() != 1 {
		t.Fatal("详情或能力缓存失败")
	}
	reply = execute(model.OperationRequest{ID: "failed", Kind: "service_details", Target: "failed.service"})
	if reply.Error == nil || reply.Error.Kind != "permission_denied" || strings.Contains(reply.Error.Message, "test-private-marker") {
		t.Fatal("EOF 后失败状态丢失或错误泄露")
	}
	done := make(chan model.OperationReply, 1)
	go func() {
		reply, _ := manager.ExecuteOperation(context.Background(), snapshot.ID, snapshot.Generation, model.OperationRequest{ID: "action", Kind: "service_action", Target: "sshd.service", Action: "restart"})
		done <- reply
	}()
	challenge := waitOperationChallenge(t, manager, snapshot, "action")
	answer := model.OperationAnswer{SessionID: snapshot.ID, Generation: snapshot.Generation, OperationID: "action", ChallengeID: "stale", Password: "test-sudo-password"}
	if manager.AnswerOperation(answer) == nil {
		t.Fatal("过期 sudo 挑战被接受")
	}
	answer.ChallengeID = challenge.ID
	if err = manager.AnswerOperation(answer); err != nil {
		t.Fatal(err)
	}
	if manager.AnswerOperation(answer) == nil {
		t.Fatal("sudo 答案重复提交")
	}
	select {
	case reply = <-done:
		if reply.Error != nil || reply.Output != "操作已完成" || strings.Contains(reply.Output, "test-sudo-password") {
			t.Fatal("sudo 动作失败或凭据泄露")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("sudo 动作未完成")
	}
	if !<-credentials {
		t.Fatal("sudo 凭据未通过对应通道传送")
	}
	if all, _ := manager.OperationChallenges(snapshot.ID, snapshot.Generation); len(all) != 0 {
		t.Fatal("sudo 挑战未清理")
	}
	if _, err = manager.ReadDirectory(context.Background(), snapshot.ID, snapshot.Generation, server.root); err != nil {
		t.Fatal("运维污染了 SFTP", err)
	}
}
func waitOperationChallenge(t *testing.T, m *Manager, s model.SessionSnapshot, id string) model.OperationChallenge {
	t.Helper()
	deadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(deadline) {
		values, err := m.OperationChallenges(s.ID, s.Generation)
		if err != nil {
			t.Fatal(err)
		}
		for _, value := range values {
			if value.OperationID == id {
				return value
			}
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("sudo 挑战未出现")
	return model.OperationChallenge{}
}
func TestOperationsCancellationLimitsAndRecovery(t *testing.T) {
	started := make(chan struct{}, 8)
	release := make(chan struct{})
	defer close(release)
	server := startLoopbackHooks(t, nil, func(ch ssh.Channel, req *ssh.Request) bool {
		var request struct{ Command string }
		if ssh.Unmarshal(req.Payload, &request) != nil {
			return false
		}
		if request.Command == remoteops.ProbeCommand {
			_ = req.Reply(true, nil)
			_, _ = io.WriteString(ch, operationCaps)
			_, _ = ch.SendRequest("exit-status", false, ssh.Marshal(struct{ Status uint32 }{0}))
			return true
		}
		if strings.Contains(request.Command, "flood.service") {
			_ = req.Reply(true, nil)
			_, _ = io.WriteString(ch, strings.Repeat("x", 9<<20))
			return true
		}
		if strings.Contains(request.Command, "sudo -S") {
			_ = req.Reply(true, nil)
			_, _ = io.WriteString(ch.Stderr(), remoteops.SudoPrompt)
			_, _ = io.Copy(io.Discard, ch)
			return true
		}
		// 包含 exec 请求协商阶段的取消。
		started <- struct{}{}
		select {
		case <-release:
		case <-time.After(2 * time.Second):
		}
		return true
	})
	manager, _ := newLoopbackManager(t, server, "tester")
	opened, err := manager.Connect("loopback")
	if err != nil {
		t.Fatal(err)
	}
	snapshot := waitConnected(t, manager, opened, true)
	reply, _ := manager.ExecuteOperation(context.Background(), snapshot.ID, snapshot.Generation, model.OperationRequest{ID: "caps", Kind: "capabilities"})
	if reply.Error != nil {
		t.Fatal(reply.Error)
	}
	for _, timed := range []bool{false, true} {
		ctx, cancel := context.WithCancel(context.Background())
		if timed {
			cancel()
			ctx, cancel = context.WithTimeout(context.Background(), 40*time.Millisecond)
		}
		id := randomID()
		done := make(chan model.OperationReply, 1)
		go func() {
			r, _ := manager.ExecuteOperation(ctx, snapshot.ID, snapshot.Generation, model.OperationRequest{ID: id, Kind: "service_details", Target: "wait.service"})
			done <- r
		}()
		select {
		case <-started:
		case <-time.After(time.Second):
			t.Fatal("exec 未开始")
		}
		if !timed {
			if err = manager.CancelOperation(snapshot.ID, snapshot.Generation, id); err != nil {
				t.Fatal(err)
			}
			s, _ := manager.Get(snapshot.ID, snapshot.Generation)
			s.mu.Lock()
			remaining := len(s.operations)
			s.mu.Unlock()
			if remaining != 0 {
				t.Fatal("取消接口返回时尚未释放运维槽")
			}
		}
		select {
		case reply = <-done:
			want := "cancelled"
			if timed {
				want = "timeout"
			}
			if reply.Error == nil || reply.Error.Kind != want {
				t.Fatal("取消或超时分类错误")
			}
		case <-time.After(1500 * time.Millisecond):
			t.Fatal("exec 协商取消未收敛")
		}
		cancel()
	}
	reply, _ = manager.ExecuteOperation(context.Background(), snapshot.ID, snapshot.Generation, model.OperationRequest{ID: "flood", Kind: "service_details", Target: "flood.service"})
	if reply.Error == nil || reply.Error.Kind != "output_limit" {
		t.Fatal("输出上限未生效")
	}
	done := make(chan model.OperationReply, 1)
	go func() {
		r, _ := manager.ExecuteOperation(context.Background(), snapshot.ID, snapshot.Generation, model.OperationRequest{ID: "sudo-cancel", Kind: "process_action", PID: 42, Action: "terminate"})
		done <- r
	}()
	challenge := waitOperationChallenge(t, manager, snapshot, "sudo-cancel")
	if err = manager.AnswerOperation(model.OperationAnswer{SessionID: snapshot.ID, Generation: snapshot.Generation, OperationID: "sudo-cancel", ChallengeID: challenge.ID, Cancel: true}); err != nil {
		t.Fatal(err)
	}
	select {
	case reply = <-done:
		if reply.Error == nil || reply.Error.Kind != "cancelled" {
			t.Fatal("sudo 取消失败")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("sudo 取消未结束")
	}
	if _, err = manager.ReadDirectory(context.Background(), snapshot.ID, snapshot.Generation, server.root); err != nil {
		t.Fatal("取消关闭宿主 SSH", err)
	}
	s, _ := manager.Get(snapshot.ID, snapshot.Generation)
	s.mu.Lock()
	pending := len(s.operations)
	s.mu.Unlock()
	if pending != 0 {
		t.Fatal("运维槽未释放")
	}
	reply, _ = manager.ExecuteOperation(context.Background(), snapshot.ID, "stale", model.OperationRequest{ID: "stale", Kind: "status"})
	if reply.Error == nil || reply.Error.Kind != "disconnected" {
		t.Fatal("旧连接代次被接受")
	}
}
