//! 独立于数据查询的能力快照；负面探测结果可由用户重新探测恢复。

use super::{OperationsDomain, OperationsError};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationsCapabilities {
    pub linux: bool,
    pub systemd: bool,
    pub ps: bool,
    pub proc: bool,
    pub ss: bool,
    pub proc_net: bool,
    pub docker_daemon: bool,
    pub compose: bool,
}

impl OperationsCapabilities {
    pub fn availability(&self, domain: OperationsDomain, compose: bool) -> Availability {
        use Availability::*;
        if !self.linux {
            return Unavailable;
        }
        match domain {
            OperationsDomain::Capabilities => Available,
            OperationsDomain::Services => {
                if self.systemd {
                    Available
                } else {
                    Unavailable
                }
            }
            OperationsDomain::Processes => match (self.ps, self.proc) {
                (true, true) => Available,
                (false, false) => Unavailable,
                _ => Degraded,
            },
            OperationsDomain::Network => match (self.ss, self.proc_net) {
                (true, true) => Available,
                (false, false) => Unavailable,
                _ => Degraded,
            },
            OperationsDomain::Docker => {
                if self.docker_daemon && (!compose || self.compose) {
                    Available
                } else {
                    Unavailable
                }
            }
        }
    }
}

// 每项独立执行并仅输出布尔值；CLI 存在不代表 daemon/system bus 可访问。
pub const PROBE_COMMAND: &str = r#"export LC_ALL=C
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
"#;

pub fn parse_capabilities(stdout: &str) -> Result<OperationsCapabilities, OperationsError> {
    serde_json::from_str(stdout).map_err(|_| OperationsError::parse("能力探测响应无效或不完整"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn probe_shell_checks_backends_independently_without_leaking_output() {
        let fixture = r#"uname() { printf 'Linux\n'; }
systemctl() { printf 'private-error\n' >&2; return 1; }
ps() { return 0; }
ss() { return 1; }
docker() { if [ "$1" = compose ]; then return 0; else printf 'daemon-unavailable\n' >&2; return 1; fi; }
"#;
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("{fixture}{PROBE_COMMAND}"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let caps = parse_capabilities(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
        assert!(caps.linux && caps.ps && caps.compose);
        assert!(!caps.systemd && !caps.ss && !caps.docker_daemon);
    }

    #[test]
    fn independent_capabilities_preserve_partial_support() {
        let mut caps = parse_capabilities(r#"{"linux":true,"systemd":false,"ps":true,"proc":false,"ss":false,"proc_net":true,"docker_daemon":false,"compose":true}"#).unwrap();
        assert_eq!(
            caps.availability(OperationsDomain::Services, false),
            Availability::Unavailable
        );
        assert_eq!(
            caps.availability(OperationsDomain::Processes, false),
            Availability::Degraded
        );
        assert_eq!(
            caps.availability(OperationsDomain::Network, false),
            Availability::Degraded
        );
        assert_eq!(
            caps.availability(OperationsDomain::Docker, true),
            Availability::Unavailable
        );
        caps.docker_daemon = true;
        assert_eq!(
            caps.availability(OperationsDomain::Docker, true),
            Availability::Available
        );
        caps.compose = false;
        assert_eq!(
            caps.availability(OperationsDomain::Docker, false),
            Availability::Available
        );
        assert_eq!(
            caps.availability(OperationsDomain::Docker, true),
            Availability::Unavailable
        );
        caps.linux = false;
        assert_eq!(
            caps.availability(OperationsDomain::Processes, false),
            Availability::Unavailable
        );
        assert!(parse_capabilities("{}").is_err());
        assert!(parse_capabilities("not json").is_err());
    }
}
