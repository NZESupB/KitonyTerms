package remoteops

import (
	"kitonyterms/internal/model"
	"strings"
	"testing"
)

const capsJSON = `{"linux":true,"systemd":true,"ps":true,"proc":false,"ss":false,"proc_net":true,"docker_daemon":true,"compose":false}`

func TestCommandValidationAndSecretProjection(t *testing.T) {
	cases := []model.OperationRequest{
		{Kind: "service_action", Target: "ssh.service; touch /tmp/x", Action: "stop"},
		{Kind: "service_action", Target: "ssh.service", Action: "status;echo x"},
		{Kind: "docker_action", Resource: "containers", Target: "--all", Action: "remove"},
		{Kind: "docker_action", Resource: "images", Target: "sha256:abc", Action: "start"},
		{Kind: "process_action", PID: 0, Action: "kill"},
		{Kind: "process_action", PID: 4294967295, Action: "kill"},
		{Kind: "docker_storage", Targets: []string{"abc", "abc"}},
		{Kind: "docker_storage", Targets: make([]string, 65)},
		{Kind: "docker_stats", Target: "$(uname)"},
		{Kind: "shell", Target: "whoami"},
	}
	for _, r := range cases {
		r.ID = "request"
		if _, err := Command(r); err == nil {
			t.Fatalf("非法请求被接受: %s", r.Kind)
		}
	}
	for _, r := range []model.OperationRequest{{Kind: "service_action", Target: "sshd@host.service", Action: "restart"}, {Kind: "process_action", PID: 42, Action: "terminate"}, {Kind: "docker_action", Resource: "containers", Target: "abc", Action: "restart"}, {Kind: "docker_action", Resource: "compose", Target: "project", Action: "remove"}, {Kind: "docker_storage", Targets: []string{"abc", "def"}}, {Kind: "docker_image_sizes", Targets: []string{"sha256:abc"}}} {
		r.ID = "request"
		command, err := Command(r)
		if err != nil || command == "" {
			t.Fatal(err)
		}
	}
	for _, resource := range []string{"containers", "images", "volumes", "networks"} {
		command, err := Command(model.OperationRequest{ID: "request", Kind: "docker_details", Resource: resource, Target: "abc"})
		if err != nil || strings.Contains(command, ".Config") {
			t.Fatal("Docker 详情读取机密字段", err)
		}
	}
	command, _ := Command(model.OperationRequest{ID: "request", Kind: "processes"})
	if strings.Contains(command, "args") || strings.Contains(command, "env") || !strings.Contains(command, "comm=") {
		t.Fatal("读取了进程参数")
	}
}
func TestTypedParsersAndCapabilities(t *testing.T) {
	r := model.OperationRequest{ID: "request", Kind: "capabilities"}
	reply, err := Parse(r, capsJSON)
	if err != nil {
		t.Fatal(err)
	}
	if Supported(*reply.Capabilities, model.OperationRequest{Kind: "network"}) || Supported(*reply.Capabilities, model.OperationRequest{Kind: "docker_list", Resource: "compose"}) || !Supported(*reply.Capabilities, model.OperationRequest{Kind: "processes"}) {
		t.Fatal("把 proc 可读误认为已实现列表后端")
	}
	for _, text := range []string{"{}", strings.Replace(capsJSON, `"ps":true`, `"ps":null`, 1), capsJSON + "{}", "not json"} {
		if _, err = Parse(r, text); err == nil {
			t.Fatal("畸形能力响应被接受")
		}
	}
	reply, err = Parse(model.OperationRequest{Kind: "services"}, "sshd.service loaded active running SSH server\n")
	if err != nil || reply.Services[0].Description != "SSH server" {
		t.Fatal(err)
	}
	reply, err = Parse(model.OperationRequest{Kind: "processes"}, "42 1 1000 S 234.5 1.5 9007199254740993 99 01:02 command name\n")
	if err != nil || reply.Processes[0].CPUPercent != 234.5 || reply.Processes[0].RSSKiB != "9007199254740993" || reply.Processes[0].Command != "command name" {
		t.Fatal(err)
	}
	reply, err = Parse(model.OperationRequest{Kind: "network"}, "tcp LISTEN 0 128 [::]:22 *:*\n")
	if err != nil || reply.Network[0].OwnerAvailable {
		t.Fatal(err)
	}
	for _, sample := range []struct{ kind, text string }{{"services", "sshd.service active"}, {"processes", "42 1 1 S NaN 1 1 1 00:01 ps"}, {"processes", "-42 1 1 S 1 1 1 1 00:01 ps"}, {"network", "tcp LISTEN"}, {"docker_stats", `{"ID":"abc","CPUPerc":"NaN%"}`}, {"status", string([]byte{0xff})}} {
		if _, err = Parse(model.OperationRequest{Kind: sample.kind}, sample.text); err == nil {
			t.Fatalf("畸形响应被接受 %s", sample.kind)
		}
	}
}
func TestDockerUnknownSizesAndMulticore(t *testing.T) {
	reply, err := Parse(model.OperationRequest{Kind: "docker_stats"}, `{"ID":"abc","CPUPerc":"234.5%","MemUsage":"1.5MiB / 2GiB","MemPerc":"0.1%"}
{"ID":"def","CPUPerc":"--","MemUsage":"--","MemPerc":"--"}`)
	if err != nil || *reply.Runtime[0].CPUPercent != 234.5 || reply.Runtime[0].MemoryUsedBytes != "1572864" || reply.Runtime[0].MemoryLimitBytes != "2147483648" || reply.Runtime[1].CPUPercent != nil || reply.Runtime[1].MemoryUsedBytes != "" {
		t.Fatal(err)
	}
	reply, err = Parse(model.OperationRequest{Kind: "docker_storage"}, `{"id":"abc","image_id":"sha256:def","size_rw":0,"size_rootfs":-1}`)
	if err != nil || reply.Storage[0].SizeRW != "0" || reply.Storage[0].SizeRootFS != "" {
		t.Fatal(err)
	}
	if _, err = Parse(model.OperationRequest{Kind: "docker_storage"}, `{"id":"abc","image_id":"def","size_rw":-2}`); err == nil {
		t.Fatal("非法大小被接受")
	}
	reply, err = Parse(model.OperationRequest{Kind: "docker_list", Resource: "compose"}, `[{"Name":"web","Status":"running(1)","ConfigFiles":"/tmp/compose.yml"}]`)
	if err != nil || reply.Docker[0].ID != "web" {
		t.Fatal(err)
	}
	for _, resource := range []string{"containers", "images", "volumes", "networks"} {
		if _, err = Parse(model.OperationRequest{Kind: "docker_list", Resource: resource}, `{}`); err == nil {
			t.Fatal("空 Docker 标识被接受")
		}
	}
	failure := CommandFailure(1, "Permission denied private-password")
	if failure.Kind != "permission_denied" || strings.Contains(failure.Message, "private-password") {
		t.Fatal("远端错误泄露")
	}
}
