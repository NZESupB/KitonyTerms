//! 类型化的 Linux 远程运维模型、命令白名单与结果解析器。
//!
//! 本模块不接受任意 shell 文本；调用方只能选择受支持的查询或管理动作。

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::oneshot;

mod capabilities;
pub use capabilities::{parse_capabilities, Availability, OperationsCapabilities, PROBE_COMMAND};

use crate::session::{AuthChallenge, FromCore, SessionEventSender, SessionId};
use crate::ssh::ChannelCloseGuard;

/// UI 全局分配且不在重连时复用的运维操作标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OperationId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationsDomain {
    Capabilities,
    Services,
    Processes,
    Network,
    Docker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockerResource {
    Compose,
    Containers,
    Images,
    Volumes,
    Networks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DockerAction {
    Start,
    Stop,
    Restart,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessAction {
    Terminate,
    Kill,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationsRequest {
    ProbeCapabilities,
    Refresh(OperationsDomain),
    DockerList(DockerResource),
    DockerInspect {
        resource: DockerResource,
        id: String,
    },
    DockerLogs {
        container: String,
        tail: u16,
    },
    DockerStats {
        container: Option<String>,
    },
    DockerAction {
        resource: DockerResource,
        id: String,
        action: DockerAction,
    },
    ServiceDetails {
        name: String,
    },
    ServiceLogs {
        name: String,
        lines: u16,
    },
    ServiceAction {
        name: String,
        action: ServiceAction,
    },
    ProcessDetails {
        pid: u32,
    },
    ProcessAction {
        pid: u32,
        action: ProcessAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationsErrorKind {
    Cancelled,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerImage {
    pub id: String,
    pub repository: String,
    pub tag: String,
    pub size: String,
    pub created: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerVolume {
    pub name: String,
    pub driver: String,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerNetwork {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerComposeProject {
    pub name: String,
    pub status: String,
    pub config_files: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OperationsResult {
    Capabilities(OperationsCapabilities),
    Services(Arc<[ServiceSummary]>),
    Processes(Arc<[ProcessSummary]>),
    NetworkConnections(Arc<[NetworkConnection]>),
    DockerContainers(Arc<[DockerContainer]>),
    DockerImages(Arc<[DockerImage]>),
    DockerVolumes(Arc<[DockerVolume]>),
    DockerNetworks(Arc<[DockerNetwork]>),
    DockerCompose(Arc<[DockerComposeProject]>),
    Details { title: String, content: String },
    Action { message: String },
}

pub const QUERY_TIMEOUT: Duration = Duration::from_secs(12);
const QUERY_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const STDERR_OUTPUT_LIMIT: usize = 64 * 1024;

/// 返回固定的、无参数的快照命令。筛选、排序和分页均必须在本地完成。
pub fn read_command(domain: OperationsDomain) -> &'static str {
    match domain {
        OperationsDomain::Capabilities => PROBE_COMMAND,
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

impl OperationsRequest {
    pub fn domain(&self) -> OperationsDomain {
        match self {
            Self::ProbeCapabilities => OperationsDomain::Capabilities,
            Self::Refresh(domain) => *domain,
            Self::DockerList(_)
            | Self::DockerInspect { .. }
            | Self::DockerLogs { .. }
            | Self::DockerStats { .. }
            | Self::DockerAction { .. } => OperationsDomain::Docker,
            Self::ServiceDetails { .. } | Self::ServiceLogs { .. } | Self::ServiceAction { .. } => {
                OperationsDomain::Services
            }
            Self::ProcessDetails { .. } | Self::ProcessAction { .. } => OperationsDomain::Processes,
        }
    }
}

pub fn request_requires_sudo(request: &OperationsRequest) -> bool {
    matches!(
        request,
        OperationsRequest::DockerAction { .. }
            | OperationsRequest::ServiceAction { .. }
            | OperationsRequest::ProcessAction { .. }
    )
}

/// Build a command from the typed request. User-provided identifiers are validated and
/// quoted before interpolation; callers never pass arbitrary shell text here.
pub fn command_for_request(request: &OperationsRequest) -> Result<String, OperationsError> {
    match request {
        OperationsRequest::ProbeCapabilities => Ok(PROBE_COMMAND.to_string()),
        OperationsRequest::Refresh(domain) => Ok(read_command(*domain).to_string()),
        OperationsRequest::DockerList(resource) => Ok(docker_list_command(*resource)),
        OperationsRequest::DockerInspect { resource, id } => {
            let id = validate_resource_id(id)?;
            Ok(format!(
                "docker {} inspect {}",
                docker_resource_command(*resource),
                shell_quote(id)
            ))
        }
        OperationsRequest::DockerLogs { container, tail } => {
            let container = validate_resource_id(container)?;
            let tail = (*tail).clamp(1, 10_000);
            Ok(format!(
                "docker logs --tail {tail} {}",
                shell_quote(container)
            ))
        }
        OperationsRequest::DockerStats { container } => {
            let target = container
                .as_deref()
                .map(validate_resource_id)
                .transpose()?
                .map(shell_quote)
                .unwrap_or_else(|| "--all".to_string());
            Ok(format!(
                "docker stats --no-stream --no-trunc --format '{{{{json .}}}}' {target}"
            ))
        }
        OperationsRequest::DockerAction {
            resource,
            id,
            action,
        } => {
            let id = validate_resource_id(id)?;
            let action = match action {
                DockerAction::Start => "start",
                DockerAction::Stop => "stop",
                DockerAction::Restart => "restart",
                DockerAction::Remove => "rm",
            };
            let command = match resource {
                DockerResource::Compose => match action {
                    "start" => compose_action_command(id, "up -d"),
                    "stop" => compose_action_command(id, "stop"),
                    "restart" => compose_action_command(id, "restart"),
                    "rm" => compose_action_command(id, "down"),
                    _ => unreachable!(),
                },
                DockerResource::Containers => format!(
                    "sudo -S -p '__KT_SUDO_PROMPT__' docker container {action} {}",
                    shell_quote(id)
                ),
                DockerResource::Images if matches!(action, "rm") => {
                    format!(
                        "sudo -S -p '__KT_SUDO_PROMPT__' docker image rm {}",
                        shell_quote(id)
                    )
                }
                DockerResource::Volumes if matches!(action, "rm") => {
                    format!(
                        "sudo -S -p '__KT_SUDO_PROMPT__' docker volume rm {}",
                        shell_quote(id)
                    )
                }
                DockerResource::Networks if matches!(action, "rm") => {
                    format!(
                        "sudo -S -p '__KT_SUDO_PROMPT__' docker network rm {}",
                        shell_quote(id)
                    )
                }
                _ => {
                    return Err(OperationsError::new(
                        OperationsErrorKind::UnsupportedBackend,
                        "该 Docker 资源不支持此动作",
                    ))
                }
            };
            Ok(command)
        }
        OperationsRequest::ServiceDetails { name } => Ok(format!(
            "LC_ALL=C systemctl show --no-pager -- {}",
            shell_quote(validate_service_name(name)?)
        )),
        OperationsRequest::ServiceLogs { name, lines } => {
            let lines = (*lines).clamp(1, 10_000);
            Ok(format!(
                "LC_ALL=C journalctl --no-pager -u {} -n {lines}",
                shell_quote(validate_service_name(name)?)
            ))
        }
        OperationsRequest::ServiceAction { name, action } => {
            let action = match action {
                ServiceAction::Start => "start",
                ServiceAction::Stop => "stop",
                ServiceAction::Restart => "restart",
                ServiceAction::Enable => "enable",
                ServiceAction::Disable => "disable",
            };
            Ok(format!(
                "sudo -S -p '__KT_SUDO_PROMPT__' systemctl {action} --no-ask-password -- {}",
                shell_quote(validate_service_name(name)?)
            ))
        }
        OperationsRequest::ProcessDetails { pid } => {
            let pid = validate_pid(*pid)?;
            Ok(format!(
                "LC_ALL=C ps -o pid=,ppid=,uid=,stat=,pcpu=,pmem=,rss=,vsz=,etime=,comm= -p {pid}"
            ))
        }
        OperationsRequest::ProcessAction { pid, action } => {
            let pid = validate_pid(*pid)?;
            let signal = match action {
                ProcessAction::Terminate => "TERM",
                ProcessAction::Kill => "KILL",
            };
            Ok(format!(
                "sudo -S -p '__KT_SUDO_PROMPT__' kill -{signal} -- {pid}"
            ))
        }
    }
}

/// Backwards-compatible accessor for the original refresh-only API.
pub fn read_command_for_request(request: &OperationsRequest) -> &'static str {
    read_command(request.domain())
}

fn docker_list_command(resource: DockerResource) -> String {
    match resource {
        DockerResource::Compose => {
            "(docker compose ls --all --format json 2>/dev/null || docker-compose ls --all --format json)".to_string()
        }
        DockerResource::Containers => read_command(OperationsDomain::Docker).to_string(),
        DockerResource::Images => "docker image ls --no-trunc --format '{{json .}}'".to_string(),
        DockerResource::Volumes => "docker volume ls --format '{{json .}}'".to_string(),
        DockerResource::Networks => "docker network ls --format '{{json .}}'".to_string(),
    }
}

fn compose_action_command(project: &str, action: &str) -> String {
    format!(
        "if docker compose version >/dev/null 2>&1; then sudo -S -p '__KT_SUDO_PROMPT__' docker compose -p {} {action}; else sudo -S -p '__KT_SUDO_PROMPT__' docker-compose -p {} {action}; fi",
        shell_quote(project),
        shell_quote(project),
    )
}

fn docker_resource_command(resource: DockerResource) -> &'static str {
    match resource {
        DockerResource::Compose => "compose",
        DockerResource::Containers => "container",
        DockerResource::Images => "image",
        DockerResource::Volumes => "volume",
        DockerResource::Networks => "network",
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn validate_resource_id(value: &str) -> Result<&str, OperationsError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
    {
        return Err(OperationsError::parse("资源标识包含不允许的字符"));
    }
    Ok(value)
}

fn validate_service_name(value: &str) -> Result<&str, OperationsError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 256
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'@' | b'-' | b':')
        })
    {
        return Err(OperationsError::parse("服务名包含不允许的字符"));
    }
    Ok(value)
}

fn validate_pid(pid: u32) -> Result<u32, OperationsError> {
    (pid > 0)
        .then_some(pid)
        .ok_or_else(|| OperationsError::parse("PID 无效"))
}

/// 读取结构化结果；终态由会话统一发送，取消任务时 guard 负责关闭通道。
pub(crate) async fn execute_request(
    id: SessionId,
    operation_id: OperationId,
    request: OperationsRequest,
    channel: russh::Channel<russh::client::Msg>,
    out: SessionEventSender,
    mut sudo_response: Option<oneshot::Receiver<Option<String>>>,
) -> Result<OperationsResult, OperationsError> {
    let domain = request.domain();
    let mut channel = ChannelCloseGuard::new(channel);
    let mut sudo_prompt_buffer = Vec::new();
    let result = tokio::time::timeout(QUERY_TIMEOUT, async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_status = None;
        let mut disconnected = false;
        loop {
            let message = channel.wait().await;
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
                    if sudo_response.is_some() {
                        sudo_prompt_buffer.extend_from_slice(&data);
                        let keep = b"__KT_SUDO_PROMPT__".len() * 2;
                        if sudo_prompt_buffer.len() > keep {
                            let start = sudo_prompt_buffer.len() - keep;
                            sudo_prompt_buffer.drain(..start);
                        }
                    }
                    if sudo_prompt_buffer
                        .windows(b"__KT_SUDO_PROMPT__".len())
                        .any(|window| window == b"__KT_SUDO_PROMPT__")
                    {
                        if let Some(response_rx) = sudo_response.take() {
                            if out
                                .try_send(FromCore::AuthChallenge {
                                    id,
                                    generation: out.generation(),
                                    challenge: AuthChallenge::Sudo {
                                        operation_id,
                                        prompt: "需要 sudo 密码才能执行此操作".to_string(),
                                    },
                                })
                                .is_err()
                            {
                                return Err(OperationsError::new(
                                    OperationsErrorKind::Busy,
                                    "sudo 认证挑战无法投递",
                                ));
                            }
                            let response =
                                tokio::time::timeout(Duration::from_secs(45), response_rx)
                                    .await
                                    .ok()
                                    .and_then(Result::ok)
                                    .flatten();
                            let Some(password) = response else {
                                return Err(OperationsError::new(
                                    OperationsErrorKind::Cancelled,
                                    "sudo 认证已取消",
                                ));
                            };
                            channel
                                .data(format!("{password}\n").as_bytes())
                                .await
                                .map_err(|_| {
                                    OperationsError::new(
                                        OperationsErrorKind::Disconnected,
                                        "sudo 密码无法发送",
                                    )
                                })?;
                            sudo_response = None;
                        }
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
            let kind = if stderr_text.contains("Permission denied")
                || stderr_text.contains("password is required")
                || stderr_text.contains("a terminal is required")
            {
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
        parse_request_result(&request, &stdout)
    })
    .await;

    // Channel::drop 不会替普通 Channel 发送 CHANNEL_CLOSE；无论查询成功、失败、
    // 超时还是任务被取消，先显式关闭远端 exec 通道再向 UI 发送终态。
    channel.close().await;

    result.unwrap_or_else(|_| {
        Err(OperationsError::new(
            OperationsErrorKind::Timeout,
            "远程查询超时（12 秒）",
        ))
    })
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

#[derive(Deserialize)]
struct DockerImageRow {
    #[serde(rename = "ID")]
    id: Option<String>,
    #[serde(rename = "Repository")]
    repository: Option<String>,
    #[serde(rename = "Tag")]
    tag: Option<String>,
    #[serde(rename = "Size")]
    size: Option<String>,
    #[serde(rename = "CreatedSince")]
    created: Option<String>,
}

pub fn parse_docker_images(stdout: &str) -> Result<Vec<DockerImage>, OperationsError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let row: DockerImageRow = serde_json::from_str(line)
                .map_err(|_| OperationsError::parse("Docker 镜像 JSON 解析失败"))?;
            let id = row
                .id
                .filter(|value| !value.is_empty())
                .ok_or_else(|| OperationsError::parse("Docker 镜像 ID 缺失"))?;
            Ok(DockerImage {
                id,
                repository: row.repository.unwrap_or_else(|| "<none>".to_string()),
                tag: row.tag.unwrap_or_else(|| "<none>".to_string()),
                size: row.size.unwrap_or_default(),
                created: row.created.unwrap_or_default(),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct DockerVolumeRow {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Driver")]
    driver: Option<String>,
    #[serde(rename = "Scope")]
    scope: Option<String>,
}

pub fn parse_docker_volumes(stdout: &str) -> Result<Vec<DockerVolume>, OperationsError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let row: DockerVolumeRow = serde_json::from_str(line)
                .map_err(|_| OperationsError::parse("Docker 卷 JSON 解析失败"))?;
            let name = row
                .name
                .filter(|value| !value.is_empty())
                .ok_or_else(|| OperationsError::parse("Docker 卷名称缺失"))?;
            Ok(DockerVolume {
                name,
                driver: row.driver.unwrap_or_default(),
                scope: row.scope.unwrap_or_default(),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct DockerNetworkRow {
    #[serde(rename = "ID")]
    id: Option<String>,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Driver")]
    driver: Option<String>,
    #[serde(rename = "Scope")]
    scope: Option<String>,
}

pub fn parse_docker_networks(stdout: &str) -> Result<Vec<DockerNetwork>, OperationsError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let row: DockerNetworkRow = serde_json::from_str(line)
                .map_err(|_| OperationsError::parse("Docker 网络 JSON 解析失败"))?;
            let id = row
                .id
                .filter(|value| !value.is_empty())
                .ok_or_else(|| OperationsError::parse("Docker 网络 ID 缺失"))?;
            Ok(DockerNetwork {
                id,
                name: row.name.unwrap_or_default(),
                driver: row.driver.unwrap_or_default(),
                scope: row.scope.unwrap_or_default(),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct DockerComposeRow {
    #[serde(alias = "Name", alias = "name")]
    name: Option<String>,
    #[serde(alias = "Status", alias = "status")]
    status: Option<String>,
    #[serde(alias = "ConfigFiles", alias = "config_files")]
    config_files: Option<String>,
}

pub fn parse_docker_compose(stdout: &str) -> Result<Vec<DockerComposeProject>, OperationsError> {
    let text = stdout.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<DockerComposeRow> = if text.starts_with('[') {
        serde_json::from_str(text)
            .map_err(|_| OperationsError::parse("Compose 项目 JSON 解析失败"))?
    } else {
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .map_err(|_| OperationsError::parse("Compose 项目 JSON 解析失败"))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            row.name.map(|name| DockerComposeProject {
                name,
                status: row.status.unwrap_or_default(),
                config_files: row.config_files.unwrap_or_default(),
            })
        })
        .collect())
}

pub fn parse_request_result(
    request: &OperationsRequest,
    stdout: &str,
) -> Result<OperationsResult, OperationsError> {
    match request {
        OperationsRequest::ProbeCapabilities
        | OperationsRequest::Refresh(OperationsDomain::Capabilities) => {
            Ok(OperationsResult::Capabilities(parse_capabilities(stdout)?))
        }
        OperationsRequest::Refresh(OperationsDomain::Services) => {
            Ok(OperationsResult::Services(parse_services(stdout)?.into()))
        }
        OperationsRequest::Refresh(OperationsDomain::Processes) => {
            Ok(OperationsResult::Processes(parse_processes(stdout)?.into()))
        }
        OperationsRequest::Refresh(OperationsDomain::Network) => Ok(
            OperationsResult::NetworkConnections(parse_network(stdout)?.into()),
        ),
        OperationsRequest::Refresh(OperationsDomain::Docker)
        | OperationsRequest::DockerList(DockerResource::Containers) => Ok(
            OperationsResult::DockerContainers(parse_docker_containers(stdout)?.into()),
        ),
        OperationsRequest::DockerList(DockerResource::Images) => Ok(
            OperationsResult::DockerImages(parse_docker_images(stdout)?.into()),
        ),
        OperationsRequest::DockerList(DockerResource::Volumes) => Ok(
            OperationsResult::DockerVolumes(parse_docker_volumes(stdout)?.into()),
        ),
        OperationsRequest::DockerList(DockerResource::Networks) => Ok(
            OperationsResult::DockerNetworks(parse_docker_networks(stdout)?.into()),
        ),
        OperationsRequest::DockerList(DockerResource::Compose) => Ok(
            OperationsResult::DockerCompose(parse_docker_compose(stdout)?.into()),
        ),
        OperationsRequest::DockerInspect { resource, id } => Ok(OperationsResult::Details {
            title: format!("Docker {:?} {id}", resource),
            content: stdout.trim().to_string(),
        }),
        OperationsRequest::DockerLogs { container, .. } => Ok(OperationsResult::Details {
            title: format!("Docker logs {container}"),
            content: stdout.to_string(),
        }),
        OperationsRequest::DockerStats { .. } => Ok(OperationsResult::Details {
            title: "Docker stats".to_string(),
            content: stdout.to_string(),
        }),
        OperationsRequest::ServiceDetails { name } => Ok(OperationsResult::Details {
            title: format!("Service {name}"),
            content: stdout.to_string(),
        }),
        OperationsRequest::ServiceLogs { name, .. } => Ok(OperationsResult::Details {
            title: format!("Service logs {name}"),
            content: stdout.to_string(),
        }),
        OperationsRequest::ProcessDetails { pid } => Ok(OperationsResult::Details {
            title: format!("Process {pid}"),
            content: stdout.to_string(),
        }),
        OperationsRequest::DockerAction { .. }
        | OperationsRequest::ServiceAction { .. }
        | OperationsRequest::ProcessAction { .. } => Ok(OperationsResult::Action {
            message: if stdout.trim().is_empty() {
                "操作已完成".to_string()
            } else {
                stdout.trim().to_string()
            },
        }),
    }
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

    #[test]
    fn docker_resource_parsers_keep_typed_fields() {
        let images = parse_docker_images(
            r#"{"ID":"sha256:abc","Repository":"kitony","Tag":"latest","Size":"12MB","CreatedSince":"2 hours ago"}"#,
        )
        .unwrap();
        assert_eq!(images[0].repository, "kitony");
        let volumes =
            parse_docker_volumes(r#"{"Name":"data","Driver":"local","Scope":"local"}"#).unwrap();
        assert_eq!(volumes[0].name, "data");
        let networks = parse_docker_networks(
            r#"{"ID":"abc","Name":"bridge","Driver":"bridge","Scope":"local"}"#,
        )
        .unwrap();
        assert_eq!(networks[0].driver, "bridge");
    }

    #[test]
    fn structured_commands_validate_identifiers_and_use_sudo_for_mutations() {
        let request = OperationsRequest::DockerAction {
            resource: DockerResource::Containers,
            id: "web-1".to_string(),
            action: DockerAction::Restart,
        };
        let command = command_for_request(&request).unwrap();
        assert!(command.starts_with("sudo -S -p '__KT_SUDO_PROMPT__' docker container restart"));
        assert!(command_for_request(&OperationsRequest::ProcessAction {
            pid: 0,
            action: ProcessAction::Kill,
        })
        .is_err());
        assert!(command_for_request(&OperationsRequest::DockerLogs {
            container: "bad;id".to_string(),
            tail: 10,
        })
        .is_err());
        let compose = command_for_request(&OperationsRequest::DockerAction {
            resource: DockerResource::Compose,
            id: "web".to_string(),
            action: DockerAction::Start,
        })
        .unwrap();
        assert!(compose.contains("docker compose version"));
        assert!(compose.contains("docker-compose -p 'web' up -d"));
    }

    #[test]
    fn compose_parser_accepts_json_array_and_lines() {
        let projects = parse_docker_compose(
            r#"[{"Name":"web","Status":"running","ConfigFiles":"/srv/compose.yml"}]"#,
        )
        .unwrap();
        assert_eq!(projects[0].name, "web");
        let projects = parse_docker_compose(
            r#"{"Name":"db","Status":"exited","ConfigFiles":"/srv/db.yml"}
{"Name":"api","Status":"running","ConfigFiles":"/srv/api.yml"}"#,
        )
        .unwrap();
        assert_eq!(projects.len(), 2);
    }
}
