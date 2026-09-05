//! 类型化的 Linux 远程运维模型与只读解析器。
//!
//! 本模块不接受任意 shell 文本；调用方只能选择受支持的领域。

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{oneshot, oneshot::error::TryRecvError};

use crate::session::{FromCore, SessionEventSender, SessionId};
use crate::ssh::ChannelCloseGuard;

/// UI 全局分配且不在重连时复用的运维操作标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationsDomain {
    Services,
    Processes,
    Network,
    Docker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationsRequest {
    Refresh(OperationsDomain),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationsErrorKind {
    CommandMissing,
    UnsupportedBackend,
    PermissionDenied,
    Busy,
    Timeout,
    OutputLimitExceeded,
    ParseFailed,
    Disconnected,
    CommandFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationsError {
    pub kind: OperationsErrorKind,
    pub message: String,
}

impl OperationsError {
    pub fn new(kind: OperationsErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    fn parse(message: impl Into<String>) -> Self {
        Self {
            kind: OperationsErrorKind::ParseFailed,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSummary {
    pub name: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessSummary {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub state: String,
    pub cpu_percent: f32,
    pub memory_percent: f32,
    pub rss_kib: u64,
    pub vsz_kib: u64,
    pub elapsed: String,
    /// 只保留 comm，不读取 argv 或环境变量。
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub status: Option<String>,
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OperationsResult {
    Services(Arc<[ServiceSummary]>),
    Processes(Arc<[ProcessSummary]>),
    NetworkConnections(Arc<[NetworkConnection]>),
    DockerContainers(Arc<[DockerContainer]>),
}

pub const QUERY_TIMEOUT: Duration = Duration::from_secs(12);
const QUERY_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const STDERR_OUTPUT_LIMIT: usize = 64 * 1024;
const EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(1);

/// 仅返回固定的、无参数的只读命令。筛选、排序和分页均必须在本地完成。
pub fn read_command(domain: OperationsDomain) -> &'static str {
    match domain {
        OperationsDomain::Services => {
            "LC_ALL=C systemctl list-units --type=service --all --no-legend --plain"
        }
        OperationsDomain::Processes => {
            "LC_ALL=C ps -eo pid=,ppid=,uid=,stat=,pcpu=,pmem=,rss=,vsz=,etime=,comm= --sort=-pcpu"
        }
        OperationsDomain::Network => "LC_ALL=C ss -H -a -n -t -u -p",
        OperationsDomain::Docker => "docker container ls -a --no-trunc --format '{{json .}}'",
    }
}

pub fn read_command_for_request(request: &OperationsRequest) -> &'static str {
    match request {
        OperationsRequest::Refresh(domain) => read_command(*domain),
    }
}

/// 执行一条只读查询，并允许宿主会话在断开时主动取消。取消不会向 UI 发送迟到的
/// 失败事件，但仍会在返回前显式关闭 SSH channel。
pub(crate) async fn execute_readonly_with_cancel(
    id: SessionId,
    operation_id: OperationId,
    domain: OperationsDomain,
    channel: russh::Channel<russh::client::Msg>,
    out: SessionEventSender,
    mut cancel: Option<oneshot::Receiver<()>>,
) {
    let mut channel = ChannelCloseGuard::new(channel);
    let mut cancelled = false;
    let result = tokio::time::timeout(QUERY_TIMEOUT, async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_status = None;
        let mut disconnected = false;
        loop {
            let message = match cancel.as_mut() {
                Some(cancel_rx) => {
                    tokio::select! {
                        _ = cancel_rx => {
                            cancelled = true;
                            return Ok(None);
                        }
                        message = channel.wait() => message,
                    }
                }
                None => channel.wait().await,
            };
            match message {
                Some(russh::ChannelMsg::Data { data }) => {
                    if stdout.len().saturating_add(data.len()) > QUERY_OUTPUT_LIMIT {
                        return Err(OperationsError::new(
                            OperationsErrorKind::OutputLimitExceeded,
                            "远程查询输出超过 8 MiB 限制",
                        ));
                    }
                    stdout.extend_from_slice(&data);
                }
                Some(russh::ChannelMsg::ExtendedData { data, .. }) => {
                    if stderr.len().saturating_add(data.len()) > STDERR_OUTPUT_LIMIT {
                        return Err(OperationsError::new(
                            OperationsErrorKind::OutputLimitExceeded,
                            "远程查询错误输出超过 64 KiB 限制",
                        ));
                    }
                    stderr.extend_from_slice(&data);
                }
                Some(russh::ChannelMsg::ExitStatus {
                    exit_status: status,
                }) => {
                    exit_status = Some(status);
                }
                // SSH 服务端通常先发送 CHANNEL_EOF、再发送 exit-status。继续读取直到
                // 收到状态，否则失败命令会被误判为成功的空快照。
                Some(russh::ChannelMsg::Eof) => continue,
                Some(russh::ChannelMsg::Failure) => {
                    return Err(OperationsError::new(
                        OperationsErrorKind::CommandFailed,
                        "远程查询命令被服务器拒绝",
                    ));
                }
                Some(russh::ChannelMsg::Close) => {
                    break;
                }
                None => {
                    disconnected = true;
                    break;
                }
                Some(_) => {}
            }
        }
        let Some(exit_status) = exit_status else {
            let (kind, message) = if disconnected {
                (OperationsErrorKind::Disconnected, "SSH 运维查询通道已断开")
            } else {
                (OperationsErrorKind::CommandFailed, "远程查询未返回退出状态")
            };
            return Err(OperationsError::new(kind, message));
        };
        if exit_status != 0 {
            let stderr_text = String::from_utf8_lossy(&stderr);
            let kind = if stderr_text.contains("Permission denied") {
                OperationsErrorKind::PermissionDenied
            } else if stderr_text.contains("not found") {
                OperationsErrorKind::CommandMissing
            } else if domain == OperationsDomain::Services
                && (stderr_text.contains("System has not been booted")
                    || stderr_text.contains("Failed to connect to bus"))
            {
                OperationsErrorKind::UnsupportedBackend
            } else {
                OperationsErrorKind::CommandFailed
            };
            return Err(OperationsError::new(
                kind,
                query_failure_message(domain, kind),
            ));
        }
        let stdout = String::from_utf8(stdout).map_err(|_| {
            OperationsError::new(OperationsErrorKind::ParseFailed, "远程查询返回了无效 UTF-8")
        })?;
        let parsed = match domain {
            OperationsDomain::Services => {
                OperationsResult::Services(parse_services(&stdout)?.into())
            }
            OperationsDomain::Processes => {
                OperationsResult::Processes(parse_processes(&stdout)?.into())
            }
            OperationsDomain::Docker => {
                OperationsResult::DockerContainers(parse_docker_containers(&stdout)?.into())
            }
            OperationsDomain::Network => {
                OperationsResult::NetworkConnections(parse_network(&stdout)?.into())
            }
        };
        Ok(Some(parsed))
    })
    .await;

    // Channel::drop 不会替普通 Channel 发送 CHANNEL_CLOSE；无论查询成功、失败、
    // 超时还是任务被取消，先显式关闭远端 exec 通道再向 UI 发送终态。
    channel.close().await;

    // A disconnect can race with the final channel message. Check once more
    // after cleanup so a result is not emitted for a session being torn down.
    if !cancelled {
        if let Some(cancel_rx) = cancel.as_mut() {
            cancelled = match cancel_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Closed) => true,
                Err(TryRecvError::Empty) => false,
            };
        }
    }

    if cancelled {
        return;
    }

    let event = match result {
        Ok(Ok(Some(result))) => FromCore::OperationResult {
            id,
            operation_id,
            domain,
            result,
        },
        Ok(Ok(None)) => return,
        Ok(Err(error)) => FromCore::OperationFailed {
            id,
            operation_id,
            domain,
            error,
        },
        Err(_) => FromCore::OperationFailed {
            id,
            operation_id,
            domain,
            error: OperationsError::new(OperationsErrorKind::Timeout, "远程查询超时（12 秒）"),
        },
    };
    let _ = tokio::time::timeout(EVENT_SEND_TIMEOUT, out.send(event)).await;
}

pub fn parse_services(stdout: &str) -> Result<Vec<ServiceSummary>, OperationsError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split_whitespace();
            let name = required(&mut fields, "服务名")?;
            let load_state = required(&mut fields, "加载状态")?;
            let active_state = required(&mut fields, "运行状态")?;
            let sub_state = required(&mut fields, "子状态")?;
            Ok(ServiceSummary {
                name,
                load_state,
                active_state,
                sub_state,
                description: fields.collect::<Vec<_>>().join(" "),
            })
        })
        .collect()
}

pub fn parse_processes(stdout: &str) -> Result<Vec<ProcessSummary>, OperationsError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split_whitespace();
            let number = |value: Option<&str>, label: &str| -> Result<u64, OperationsError> {
                value
                    .ok_or_else(|| OperationsError::parse(format!("{label} 缺失")))?
                    .parse()
                    .map_err(|_| OperationsError::parse(format!("{label} 无效")))
            };
            let pid = u32::try_from(number(fields.next(), "PID")?)
                .map_err(|_| OperationsError::parse("PID 超出范围"))?;
            let ppid = u32::try_from(number(fields.next(), "PPID")?)
                .map_err(|_| OperationsError::parse("PPID 超出范围"))?;
            let uid = u32::try_from(number(fields.next(), "UID")?)
                .map_err(|_| OperationsError::parse("UID 超出范围"))?;
            let state = required(&mut fields, "状态")?;
            let cpu_percent = fields
                .next()
                .ok_or_else(|| OperationsError::parse("CPU 缺失"))?
                .parse()
                .map_err(|_| OperationsError::parse("CPU 无效"))?;
            let memory_percent = fields
                .next()
                .ok_or_else(|| OperationsError::parse("内存缺失"))?
                .parse()
                .map_err(|_| OperationsError::parse("内存无效"))?;
            let rss_kib = number(fields.next(), "RSS")?;
            let vsz_kib = number(fields.next(), "VSZ")?;
            let elapsed = required(&mut fields, "运行时间")?;
            let command = fields.collect::<Vec<_>>().join(" ");
            if command.is_empty() {
                return Err(OperationsError::parse("命令名缺失"));
            }
            Ok(ProcessSummary {
                pid,
                ppid,
                uid,
                state,
                cpu_percent,
                memory_percent,
                rss_kib,
                vsz_kib,
                elapsed,
                command,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConnection {
    pub protocol: String,
    pub state: String,
    pub local: String,
    pub peer: String,
    pub owner: Option<String>,
    pub owner_available: bool,
}

pub fn parse_network(stdout: &str) -> Result<Vec<NetworkConnection>, OperationsError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 6 {
                return Err(OperationsError::parse("网络连接字段不足"));
            }
            let protocol = fields[0];
            let state = fields[1];
            let local = fields[4];
            let peer = fields[5];
            let owner = fields[6..].join(" ");
            Ok(NetworkConnection {
                protocol: protocol.to_string(),
                state: state.to_string(),
                local: local.to_string(),
                peer: peer.to_string(),
                owner_available: !owner.is_empty(),
                owner: (!owner.is_empty()).then_some(owner),
            })
        })
        .collect()
}

fn query_failure_message(domain: OperationsDomain, kind: OperationsErrorKind) -> &'static str {
    match (domain, kind) {
        (OperationsDomain::Services, OperationsErrorKind::UnsupportedBackend) => {
            "此服务器未提供可用的 systemd system bus"
        }
        (_, OperationsErrorKind::CommandMissing) => "远程服务器缺少此功能所需的命令",
        (_, OperationsErrorKind::PermissionDenied) => "当前 SSH 用户没有读取此信息的权限",
        _ => "远程查询命令执行失败",
    }
}

#[derive(Deserialize)]
struct DockerRow {
    #[serde(rename = "ID")]
    id: Option<String>,
    #[serde(rename = "Names")]
    names: Option<String>,
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "Image")]
    image: Option<String>,
}

pub fn parse_docker_containers(stdout: &str) -> Result<Vec<DockerContainer>, OperationsError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let row: DockerRow = serde_json::from_str(line)
                .map_err(|_| OperationsError::parse("Docker JSON 解析失败"))?;
            let id = row
                .id
                .filter(|value| !value.is_empty())
                .ok_or_else(|| OperationsError::parse("Docker 容器 ID 缺失"))?;
            Ok(DockerContainer {
                name: row.names.unwrap_or_else(|| id.clone()),
                id,
                status: row.status,
                image: row.image,
            })
        })
        .collect()
}

fn required<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
    label: &str,
) -> Result<String, OperationsError> {
    fields
        .next()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| OperationsError::parse(format!("{label} 缺失")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_parser_keeps_description_with_spaces() {
        let services =
            parse_services("sshd.service loaded active running OpenSSH server daemon\n").unwrap();
        assert_eq!(services[0].name, "sshd.service");
        assert_eq!(services[0].sub_state, "running");
        assert_eq!(services[0].description, "OpenSSH server daemon");
    }

    #[test]
    fn process_parser_keeps_comm_spaces_without_reading_argv() {
        let result = parse_processes("7 1 0 S 0.1 0.2 12 34 00:01 Firefox Main Process\n").unwrap();
        assert_eq!(result[0].command, "Firefox Main Process");
    }

    #[test]
    fn service_parser_requires_sub_state_column() {
        assert!(parse_services("sshd.service loaded active\n").is_err());
    }
    #[test]
    fn docker_parser_requires_id() {
        assert!(parse_docker_containers("{\"Names\":\"web\"}\n").is_err());
    }

    #[test]
    fn network_parser_accepts_ipv6_and_missing_owner() {
        let connections = parse_network(
            "tcp LISTEN 0 4096 [::]:22 [::]:* users:((\"sshd\",pid=12,fd=3))\n\
             udp UNCONN 0 0 127.0.0.1:323 0.0.0.0:*\n",
        )
        .unwrap();
        assert_eq!(connections.len(), 2);
        assert_eq!(connections[0].local, "[::]:22");
        assert!(connections[0].owner_available);
        assert!(!connections[1].owner_available);
    }

    #[test]
    fn network_parser_rejects_malformed_nonempty_lines() {
        let result = parse_network(
            "tcp LISTEN 0 4096 127.0.0.1:22 0.0.0.0:*\n\
             malformed\n",
        );
        assert_eq!(result.unwrap_err().kind, OperationsErrorKind::ParseFailed);
    }

    #[test]
    fn docker_parser_handles_empty_list() {
        assert!(parse_docker_containers("\n").unwrap().is_empty());
    }
}
