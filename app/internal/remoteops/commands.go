// Package remoteops 提供固定命令和有界响应解析，不依赖 SSH 或 UI。
package remoteops

import (
	"fmt"
	"kitonyterms/internal/model"
	"regexp"
	"strings"
)

const ProbeCommand = `export LC_ALL=C
linux=false; systemd=false; ps_ok=false; proc_ok=false; ss_ok=false; proc_net=false; docker_daemon=false; compose=false
[ "$(uname -s 2>/dev/null)" = Linux ] && linux=true
systemctl show --property=Version --value >/dev/null 2>&1 && systemd=true
ps -eo pid=,ppid=,uid=,stat=,pcpu=,pmem=,rss=,vsz=,etime=,comm= --sort=-pcpu >/dev/null 2>&1 && ps_ok=true
[ -r /proc/self/stat ] && [ -r /proc/self/status ] && proc_ok=true
ss -H -a -n -t -u -p >/dev/null 2>&1 && ss_ok=true
[ -r /proc/net/tcp ] && [ -r /proc/net/udp ] && proc_net=true
docker info --format '{{.ServerVersion}}' >/dev/null 2>&1 && docker_daemon=true
(docker compose version >/dev/null 2>&1 || docker-compose version >/dev/null 2>&1) && compose=true
printf '{"linux":%s,"systemd":%s,"ps":%s,"proc":%s,"ss":%s,"proc_net":%s,"docker_daemon":%s,"compose":%s}\n' "$linux" "$systemd" "$ps_ok" "$proc_ok" "$ss_ok" "$proc_net" "$docker_daemon" "$compose"
`
const processCommand = "LC_ALL=C ps -eo pid=,ppid=,uid=,stat=,pcpu=,pmem=,rss=,vsz=,etime=,comm= --sort=-pcpu"
const SudoPrompt = "__KT_SUDO_PROMPT__"
const sudo = "sudo -S -p '" + SudoPrompt + "' "

var resourceID = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$`)
var serviceName = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.@:-]{0,255}$`)

func Failure(kind, message string) *model.OperationError {
	return &model.OperationError{Kind: kind, Message: message}
}
func invalid() *model.OperationError {
	return Failure("invalid_request", "运维请求或资源标识无效")
}
func quote(s string) string { return "'" + strings.ReplaceAll(s, "'", "'\\''") + "'" }
func IsAction(r model.OperationRequest) bool {
	return r.Kind == "service_action" || r.Kind == "process_action" || r.Kind == "docker_action"
}
func oneOf(s string, values ...string) bool {
	for _, v := range values {
		if s == v {
			return true
		}
	}
	return false
}
func resource(s string) string {
	return map[string]string{"containers": "container", "images": "image", "volumes": "volume", "networks": "network"}[s]
}

// 容器交互入口固定使用 /bin/sh，不接受命令文本或额外参数。
func ContainerCommand(execID, container string) (string, error) {
	if !resourceID.MatchString(execID) || !resourceID.MatchString(container) {
		return "", invalid()
	}
	return "docker exec -it " + quote(container) + " /bin/sh", nil
}

func Command(r model.OperationRequest) (string, error) {
	if r.ID == "" || len(r.ID) > 128 || !resourceID.MatchString(r.ID) || len(r.Target) > 256 || len(r.Targets) > 64 {
		return "", invalid()
	}
	if r.Kind == "capabilities" {
		return ProbeCommand, nil
	}
	switch r.Kind {
	case "status":
		return "uname -a; uptime; printf '\\n'; df -h; printf '\\n'; free -m", nil
	case "services":
		return "LC_ALL=C systemctl list-units --type=service --all --no-legend --plain --no-pager", nil
	case "processes":
		return processCommand, nil
	case "network":
		return "LC_ALL=C ss -H -a -n -t -u -p", nil
	case "service_details", "service_logs", "service_action":
		if !serviceName.MatchString(r.Target) {
			return "", invalid()
		}
		switch r.Kind {
		case "service_details":
			return "LC_ALL=C systemctl show --no-pager --property=Id,Description,LoadState,ActiveState,SubState,MainPID,UnitFileState,FragmentPath,ActiveEnterTimestamp -- " + quote(r.Target), nil
		case "service_logs":
			return fmt.Sprintf("LC_ALL=C journalctl --no-pager -u %s -n %d", quote(r.Target), min(10000, max(1, r.Lines))), nil
		default:
			if !oneOf(r.Action, "start", "stop", "restart", "enable", "disable") {
				return "", invalid()
			}
			return sudo + "systemctl " + r.Action + " --no-ask-password -- " + quote(r.Target), nil
		}
	case "process_details", "process_action":
		if r.PID == 0 || r.PID > 2147483647 {
			return "", invalid()
		}
		if r.Kind == "process_details" {
			return fmt.Sprintf("LC_ALL=C ps -o pid=,ppid=,uid=,stat=,pcpu=,pmem=,rss=,vsz=,etime=,comm= -p %d", r.PID), nil
		}
		signal := map[string]string{"terminate": "TERM", "kill": "KILL"}[r.Action]
		if signal == "" {
			return "", invalid()
		}
		return fmt.Sprintf("%skill -%s -- %d", sudo, signal, r.PID), nil
	case "docker_list":
		if r.Resource == "compose" {
			return "if docker compose version >/dev/null 2>&1; then docker compose ls --all --format json; else docker-compose ls --all --format json; fi", nil
		}
		res := resource(r.Resource)
		if res == "" {
			return "", invalid()
		}
		flags := ""
		if r.Resource == "containers" {
			flags = "-a --no-trunc "
		}
		if r.Resource == "images" {
			flags = "--no-trunc "
		}
		return "docker " + res + " ls " + flags + "--format '{{json .}}'", nil
	case "docker_logs", "docker_network", "docker_details", "docker_action":
		if !resourceID.MatchString(r.Target) {
			return "", invalid()
		}
		switch r.Kind {
		case "docker_logs":
			return fmt.Sprintf("docker logs --tail %d %s", min(10000, max(1, r.Lines)), quote(r.Target)), nil
		case "docker_network":
			return "docker container inspect --format '{{json .NetworkSettings.Networks}}' " + quote(r.Target), nil
		case "docker_details":
			res := resource(r.Resource)
			if res == "" {
				return "", invalid()
			}
			// 投影排除 Config.Env、进程参数和其它凭据字段。
			projection := map[string]string{"containers": `{"id":{{json .Id}},"name":{{json .Name}},"image":{{json .Image}},"status":{{json .State.Status}},"created":{{json .Created}},"mounts":{{json .Mounts}}}`,
				"images":   `{"id":{{json .Id}},"tags":{{json .RepoTags}},"size":{{json .Size}},"created":{{json .Created}}}`,
				"volumes":  `{"name":{{json .Name}},"driver":{{json .Driver}},"scope":{{json .Scope}},"mountpoint":{{json .Mountpoint}}}`,
				"networks": `{"id":{{json .Id}},"name":{{json .Name}},"driver":{{json .Driver}},"scope":{{json .Scope}},"ipam":{{json .IPAM}}}`}[r.Resource]
			return "docker " + res + " inspect --format " + quote(projection) + " " + quote(r.Target), nil
		default:
			action := map[string]string{"start": "start", "stop": "stop", "restart": "restart", "remove": "rm"}[r.Action]
			if action == "" {
				return "", invalid()
			}
			if r.Resource == "compose" {
				action = map[string]string{"start": "up -d", "stop": "stop", "restart": "restart", "remove": "down"}[r.Action]
				return "if docker compose version >/dev/null 2>&1; then " + sudo + "docker compose -p " + quote(r.Target) + " " + action + "; else " + sudo + "docker-compose -p " + quote(r.Target) + " " + action + "; fi", nil
			}
			res := resource(r.Resource)
			if res == "" || (r.Resource != "containers" && r.Action != "remove") {
				return "", invalid()
			}
			return sudo + "docker " + res + " " + action + " " + quote(r.Target), nil
		}
	case "docker_stats":
		target := "--all"
		if r.Target != "" {
			if !resourceID.MatchString(r.Target) {
				return "", invalid()
			}
			target = quote(r.Target)
		}
		return "LC_ALL=C docker stats --no-stream --no-trunc --format '{{json .}}' " + target, nil
	case "docker_storage", "docker_image_sizes":
		if len(r.Targets) == 0 {
			return "", invalid()
		}
		targets := make([]string, 0, len(r.Targets))
		seen := map[string]bool{}
		for _, id := range r.Targets {
			if !resourceID.MatchString(id) || seen[id] {
				return "", invalid()
			}
			seen[id] = true
			targets = append(targets, quote(id))
		}
		res, flags, projection := "container", "--size ", `{"id":{{json .Id}},"image_id":{{json .Image}},"size_rw":{{json .SizeRw}},"size_rootfs":{{json .SizeRootFs}}}`
		if r.Kind == "docker_image_sizes" {
			res = "image"
			flags = ""
			projection = `{"id":{{json .Id}},"size":{{json .Size}}}`
		}
		return "docker " + res + " inspect " + flags + "--format " + quote(projection) + " " + strings.Join(targets, " "), nil
	}
	return "", invalid()
}

func Supported(c model.OperationsCapabilities, r model.OperationRequest) bool {
	if r.Kind == "capabilities" {
		return true
	}
	if !c.Linux {
		return false
	}
	switch {
	case strings.HasPrefix(r.Kind, "service"):
		return c.Systemd
	case strings.HasPrefix(r.Kind, "process"):
		return c.PS
	case r.Kind == "network":
		return c.SS
	case strings.HasPrefix(r.Kind, "docker"):
		return c.DockerDaemon && (r.Resource != "compose" || c.Compose)
	default:
		return r.Kind == "status"
	}
}

func CommandFailure(status uint32, stderr string) *model.OperationError {
	lower := strings.ToLower(stderr)
	switch {
	case strings.Contains(lower, "permission denied"), strings.Contains(lower, "password"), strings.Contains(lower, "not in the sudoers"), strings.Contains(lower, "not allowed"), strings.Contains(lower, "terminal is required"):
		return Failure("permission_denied", "当前 SSH 用户没有执行此操作的权限，或 sudo 认证失败")
	case status == 127, strings.Contains(lower, "command not found"), strings.Contains(lower, "not found"):
		return Failure("command_missing", "远程服务器缺少此功能所需的命令或资源")
	case strings.Contains(lower, "system has not been booted"), strings.Contains(lower, "failed to connect to bus"), strings.Contains(lower, "cannot connect to the docker daemon"):
		return Failure("unsupported_backend", "远程服务后端不可用，请重新探测能力")
	default:
		return Failure("command_failed", fmt.Sprintf("远程运维命令失败（退出状态 %d）", status))
	}
}
