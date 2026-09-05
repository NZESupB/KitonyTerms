//! 服务器资源监控子任务 —— 通过一条持久 `sh` 通道周期采集远端 `/proc` 等数据。
//! 监控延迟优先通过 TCP connect 到当前 SSH 端口测量，失败时回退到已连接 SSH 通道心跳。
//!
//! Server resource monitor: drives a persistent `sh` channel, periodically writing
//! a command bundle and parsing the output into [`MonitorStats`]. CPU% and network
//! rates are computed from deltas between polls. Linux-only (`/proc`); missing
//! fields degrade gracefully. Runs in its own task so it never blocks the shell.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::sync::oneshot;

use crate::session::{FromCore, SessionEventSender, SessionId};
use crate::ssh::ChannelCloseGuard;

/// 轮询间隔。Poll interval.
const POLL: std::time::Duration = std::time::Duration::from_secs(2);
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(12);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(4);
const TCP_LATENCY_TIMEOUT: Duration = Duration::from_millis(900);
const MONITOR_IO_TIMEOUT: Duration = Duration::from_secs(4);
const MONITOR_EVENT_TIMEOUT: Duration = Duration::from_secs(1);

/// 监控子任务退出原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonitorExit {
    /// 远端正常关闭或结束,不应展示为错误。
    Stopped,
    /// 子任务已发送用户可见错误事件。
    ErrorReported,
    /// core 输出通道已关闭,无需继续通知 UI。
    ReceiverDropped,
}

/// 命令包:各段以哨兵分隔,便于切分解析。
/// Command bundle; sections delimited by sentinels for easy splitting.
const CMD: &str = "echo __KTM_BEGIN__;\
cat /proc/stat 2>/dev/null | grep '^cpu';echo __KTM_SEC__;\
cat /proc/meminfo 2>/dev/null;echo __KTM_SEC__;\
cat /proc/net/dev 2>/dev/null;echo __KTM_SEC__;\
df -P -k 2>/dev/null;echo __KTM_SEC__;\
cat /proc/loadavg 2>/dev/null;echo __KTM_SEC__;\
cat /proc/uptime 2>/dev/null;echo __KTM_SEC__;\
printf 'hostname=%s\\n' \"$(hostname 2>/dev/null)\"; printf 'arch=%s\\n' \"$(uname -m 2>/dev/null)\"; awk -F= '/^PRETTY_NAME=/{gsub(/^\"|\"$/, \"\", $2); print \"distro=\" $2; exit}' /etc/os-release 2>/dev/null;\
echo __KTM_END__\n";

const BEGIN: &str = "__KTM_BEGIN__";
const SEC: &str = "__KTM_SEC__";
const END: &str = "__KTM_END__";
const HEARTBEAT: &str = "__KTM_HEARTBEAT__";
const HEARTBEAT_CMD: &str = "printf '__KTM_HEARTBEAT__\\n'\n";

/// 单个挂载点使用情况。Disk usage for one mount point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskUsage {
    pub mount: String,
    pub used: u64,
    pub total: u64,
}

/// 远端主机的静态系统信息。只在 Monitor 通道中读取，不影响交互终端。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemInfo {
    pub hostname: String,
    pub distro: Option<String>,
    pub architecture: Option<String>,
}

/// 单个逻辑 CPU 的利用率。
#[derive(Debug, Clone, PartialEq)]
pub struct CpuCoreUsage {
    pub id: u32,
    pub percent: f32,
}

/// 单个网卡的实时收发速率。虚拟网卡由名称启发式识别，UI 可按需隐藏。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetInterfaceStats {
    pub name: String,
    pub rx_rate: u64,
    pub tx_rate: u64,
    pub is_virtual: bool,
}

/// 一次采样的服务器资源快照。A snapshot of server resource usage.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonitorStats {
    pub system: SystemInfo,
    /// CPU 使用率(0..100)。
    pub cpu_percent: f32,
    /// 远端 CPU 逻辑核心数。
    pub cpu_cores: u32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// 下行/上行速率(字节/秒)。
    pub net_rx_rate: u64,
    pub net_tx_rate: u64,
    pub load1: f32,
    pub uptime_secs: u64,
    /// 轻量心跳在已连接 SSH 通道上的往返时间，避免被重监控采样耗时放大。
    pub latency_ms: u64,
    pub disks: Vec<DiskUsage>,
    pub cpu_per_core: Vec<CpuCoreUsage>,
    pub interfaces: Vec<NetInterfaceStats>,
}

/// TCP 延迟探测目标。通常就是当前 SSH 会话的 host:port。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LatencyProbeTarget {
    host: String,
    port: u16,
}

impl LatencyProbeTarget {
    pub(crate) fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

/// CPU 累计 jiffies(busy, total),用于算增量百分比。
#[derive(Clone, Copy)]
struct CpuSample {
    busy: u64,
    total: u64,
}

/// 网络累计字节(rx, tx),用于算速率。
#[derive(Clone, Copy)]
struct NetSample {
    rx: u64,
    tx: u64,
}

#[derive(Clone, Copy)]
struct NetInterfaceSample {
    rx: u64,
    tx: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadUntil {
    Found,
    Closed,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeartbeatError {
    SendFailed,
    Closed,
    Timeout,
}

/// 监控子任务主循环。通道关闭(会话结束)即退出。
/// Monitor loop; exits when the channel closes (session ended).
pub(crate) async fn monitor_task(
    id: SessionId,
    channel: russh::Channel<russh::client::Msg>,
    latency_target: LatencyProbeTarget,
    out: SessionEventSender,
    mut cancel_rx: oneshot::Receiver<()>,
) -> MonitorExit {
    let mut channel = ChannelCloseGuard::new(channel);
    let mut prev_cpu: Option<CpuSample> = None;
    let mut prev_net: Option<NetSample> = None;
    let mut prev_cpu_cores: HashMap<u32, CpuSample> = HashMap::new();
    let mut prev_interfaces: HashMap<String, NetInterfaceSample> = HashMap::new();
    let mut prev_at: Option<Instant> = None;
    let mut buf = String::new();

    let exit = loop {
        if matches!(
            cancel_rx.try_recv(),
            Ok(()) | Err(oneshot::error::TryRecvError::Closed)
        ) {
            break MonitorExit::Stopped;
        }
        let latency_ms = match measure_latency(&mut channel, &mut buf, &latency_target).await {
            Ok(latency_ms) => latency_ms,
            Err(HeartbeatError::Closed) => break MonitorExit::Stopped,
            Err(HeartbeatError::SendFailed) => {
                if !send_monitor_event(
                    &out,
                    FromCore::MonitorError {
                        id,
                        message: "资源监控心跳发送失败".to_string(),
                    },
                )
                .await
                {
                    break MonitorExit::ReceiverDropped;
                }
                break MonitorExit::ErrorReported;
            }
            Err(HeartbeatError::Timeout) => {
                if !send_monitor_event(
                    &out,
                    FromCore::MonitorError {
                        id,
                        message: format!("资源监控心跳超时({} 秒)", HEARTBEAT_TIMEOUT.as_secs()),
                    },
                )
                .await
                {
                    break MonitorExit::ReceiverDropped;
                }
                break MonitorExit::ErrorReported;
            }
        };

        // 写入命令包。Write the command bundle.
        if matches!(
            tokio::time::timeout(MONITOR_IO_TIMEOUT, channel.data(CMD.as_bytes())).await,
            Err(_) | Ok(Err(_))
        ) {
            if !send_monitor_event(
                &out,
                FromCore::MonitorError {
                    id,
                    message: "资源监控命令发送失败".to_string(),
                },
            )
            .await
            {
                break MonitorExit::ReceiverDropped;
            }
            break MonitorExit::ErrorReported;
        }

        // 读取到 END 哨兵为止。Read until the END sentinel.
        match read_until(&mut channel, &mut buf, END, SAMPLE_TIMEOUT).await {
            ReadUntil::Found => {}
            ReadUntil::Closed => break MonitorExit::Stopped,
            ReadUntil::Timeout => {
                if !send_monitor_event(
                    &out,
                    FromCore::MonitorError {
                        id,
                        message: format!("资源监控采样超时({} 秒)", SAMPLE_TIMEOUT.as_secs()),
                    },
                )
                .await
                {
                    break MonitorExit::ReceiverDropped;
                }
                break MonitorExit::ErrorReported;
            }
        }

        let now = Instant::now();
        let elapsed = prev_at
            .map(|p| now.duration_since(p).as_secs_f64())
            .unwrap_or(0.0);
        prev_at = Some(now);

        if let Some(mut stats) = parse_block(
            &buf,
            &mut prev_cpu,
            &mut prev_net,
            &mut prev_cpu_cores,
            &mut prev_interfaces,
            elapsed,
        ) {
            stats.latency_ms = latency_ms;
            if !send_monitor_event(
                &out,
                FromCore::Monitor {
                    id,
                    stats: Box::new(stats),
                },
            )
            .await
            {
                break MonitorExit::ReceiverDropped;
            }
        } else {
            if !send_monitor_event(
                &out,
                FromCore::MonitorError {
                    id,
                    message: "资源监控采样解析失败".to_string(),
                },
            )
            .await
            {
                break MonitorExit::ReceiverDropped;
            }
            break MonitorExit::ErrorReported;
        }

        tokio::select! {
            _ = &mut cancel_rx => break MonitorExit::Stopped,
            _ = tokio::time::sleep(POLL) => {}
        }
    };

    // 关闭 sh:写 EOF 让远端进程退出。
    // Close sh by sending EOF so the remote process exits.
    let _ = tokio::time::timeout(MONITOR_IO_TIMEOUT, channel.eof()).await;
    // `russh::Channel` 的 Drop 不会替普通 channel 发送 CHANNEL_CLOSE。
    // EOF 只结束远端 stdin，仍需显式关闭通道以收敛会话资源。
    channel.close().await;
    exit
}

async fn send_monitor_event(out: &SessionEventSender, event: FromCore) -> bool {
    match tokio::time::timeout(MONITOR_EVENT_TIMEOUT, out.send(event)).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) | Err(_) => false,
    }
}

async fn measure_latency(
    channel: &mut russh::Channel<russh::client::Msg>,
    buf: &mut String,
    target: &LatencyProbeTarget,
) -> Result<u64, HeartbeatError> {
    if let Some(latency_ms) = measure_tcp_connect_latency(target).await {
        return Ok(latency_ms);
    }

    match measure_heartbeat_latency(channel, buf).await {
        Ok(latency_ms) => Ok(latency_ms),
        Err(HeartbeatError::Timeout) => {
            tracing::debug!("SSH 心跳延迟探测超时，本轮资源采样继续并标记延迟未知");
            Ok(0)
        }
        Err(error) => Err(error),
    }
}

async fn measure_tcp_connect_latency(target: &LatencyProbeTarget) -> Option<u64> {
    let started = Instant::now();
    let connect = TcpStream::connect((target.host.as_str(), target.port));

    match tokio::time::timeout(TCP_LATENCY_TIMEOUT, connect).await {
        Ok(Ok(stream)) => {
            drop(stream);
            Some(elapsed_millis(started))
        }
        Ok(Err(error)) => {
            tracing::debug!(
                host = %target.host,
                port = target.port,
                error = %error,
                "TCP 延迟探测失败，回退到 SSH 心跳"
            );
            None
        }
        Err(_) => {
            tracing::debug!(
                host = %target.host,
                port = target.port,
                "TCP 延迟探测超时，回退到 SSH 心跳"
            );
            None
        }
    }
}

async fn measure_heartbeat_latency(
    channel: &mut russh::Channel<russh::client::Msg>,
    buf: &mut String,
) -> Result<u64, HeartbeatError> {
    let started = Instant::now();
    if matches!(
        tokio::time::timeout(MONITOR_IO_TIMEOUT, channel.data(HEARTBEAT_CMD.as_bytes())).await,
        Err(_) | Ok(Err(_))
    ) {
        return Err(HeartbeatError::SendFailed);
    }

    match read_until(channel, buf, HEARTBEAT, HEARTBEAT_TIMEOUT).await {
        ReadUntil::Found => Ok(elapsed_millis(started)),
        ReadUntil::Closed => Err(HeartbeatError::Closed),
        ReadUntil::Timeout => Err(HeartbeatError::Timeout),
    }
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

async fn read_until(
    channel: &mut russh::Channel<russh::client::Msg>,
    buf: &mut String,
    needle: &str,
    timeout: Duration,
) -> ReadUntil {
    buf.clear();
    let sample_timeout = tokio::time::sleep(timeout);
    tokio::pin!(sample_timeout);

    loop {
        tokio::select! {
            _ = &mut sample_timeout => {
                return ReadUntil::Timeout;
            }
            msg = channel.wait() => {
                match msg {
                    Some(russh::ChannelMsg::Data { data }) => {
                        buf.push_str(&String::from_utf8_lossy(&data));
                        if buf.contains(needle) {
                            return ReadUntil::Found;
                        }
                    }
                    Some(russh::ChannelMsg::ExtendedData { .. }) => {}
                    Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => {
                        return ReadUntil::Closed;
                    }
                    _ => {}
                }
            }
        }
    }
}

/// 解析一次输出块。`prev_*` 在内部更新以便下次算增量。
/// Parse one output block; updates `prev_*` for next-poll deltas.
fn parse_block(
    raw: &str,
    prev_cpu: &mut Option<CpuSample>,
    prev_net: &mut Option<NetSample>,
    prev_cpu_cores: &mut HashMap<u32, CpuSample>,
    prev_interfaces: &mut HashMap<String, NetInterfaceSample>,
    elapsed: f64,
) -> Option<MonitorStats> {
    // 截取 BEGIN..END 之间,再按 SEC 切段。
    let start = raw.find(BEGIN)? + BEGIN.len();
    let end = raw.find(END)?;
    if end <= start {
        return None;
    }
    let body = &raw[start..end];
    let secs: Vec<&str> = body.split(SEC).collect();
    let get = |i: usize| secs.get(i).copied().unwrap_or("");

    let mut stats = MonitorStats::default();

    // --- CPU ---
    if let Some(cur) = parse_cpu(get(0)) {
        if let Some(prev) = *prev_cpu {
            let dt = cur.total.saturating_sub(prev.total);
            let db = cur.busy.saturating_sub(prev.busy);
            if dt > 0 {
                stats.cpu_percent = (db as f32 / dt as f32 * 100.0).clamp(0.0, 100.0);
            }
        }
        *prev_cpu = Some(cur);
    }
    stats.cpu_cores = parse_cpu_cores(get(0));
    stats.cpu_per_core = parse_cpu_core_usage(get(0), prev_cpu_cores);

    // --- MEM ---
    let (mt, ma, st, sf) = parse_meminfo(get(1));
    stats.mem_total = mt;
    stats.mem_used = mt.saturating_sub(ma);
    stats.swap_total = st;
    stats.swap_used = st.saturating_sub(sf);

    // --- NET ---
    if let Some(cur) = parse_net(get(2)) {
        if let (Some(prev), true) = (*prev_net, elapsed > 0.0) {
            let drx = cur.rx.saturating_sub(prev.rx);
            let dtx = cur.tx.saturating_sub(prev.tx);
            stats.net_rx_rate = (drx as f64 / elapsed) as u64;
            stats.net_tx_rate = (dtx as f64 / elapsed) as u64;
        }
        *prev_net = Some(cur);
    }
    stats.interfaces = parse_interfaces(get(2), prev_interfaces, elapsed);

    // --- DISK ---
    stats.disks = parse_df(get(3));

    // --- LOAD ---
    stats.load1 = get(4)
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    // --- UPTIME ---
    stats.uptime_secs = get(5)
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v as u64)
        .unwrap_or(0);

    // --- STATIC SYSTEM ---
    stats.system = parse_system_info(get(6));

    Some(stats)
}

fn parse_cpu_core_usage(s: &str, previous: &mut HashMap<u32, CpuSample>) -> Vec<CpuCoreUsage> {
    let mut current_ids = Vec::new();
    let mut usage = Vec::new();
    for line in s.lines() {
        let Some(rest) = line.strip_prefix("cpu") else {
            continue;
        };
        let Some((id, values)) = rest.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(id) = id.parse::<u32>() else {
            continue;
        };
        let values: Vec<u64> = values
            .split_whitespace()
            .filter_map(|value| value.parse().ok())
            .collect();
        if values.len() < 4 {
            continue;
        }
        let total: u64 = values.iter().sum();
        let idle = values[3].saturating_add(values.get(4).copied().unwrap_or(0));
        let sample = CpuSample {
            busy: total.saturating_sub(idle),
            total,
        };
        let percent = previous
            .get(&id)
            .and_then(|prior| {
                let total_delta = sample.total.saturating_sub(prior.total);
                (total_delta > 0).then(|| {
                    (sample.busy.saturating_sub(prior.busy) as f32 / total_delta as f32 * 100.0)
                        .clamp(0.0, 100.0)
                })
            })
            .unwrap_or(0.0);
        previous.insert(id, sample);
        current_ids.push(id);
        usage.push(CpuCoreUsage { id, percent });
    }
    previous.retain(|id, _| current_ids.contains(id));
    usage.sort_by_key(|core| core.id);
    usage
}

/// 解析 `/proc/stat` 的 `cpu ` 行 → (busy, total) jiffies。
fn parse_cpu(s: &str) -> Option<CpuSample> {
    let line = s.lines().find(|l| l.starts_with("cpu "))?;
    let vals: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|x| x.parse().ok())
        .collect();
    if vals.len() < 4 {
        return None;
    }
    let total: u64 = vals.iter().sum();
    // idle = idle + iowait(若有)。
    let idle = vals[3] + vals.get(4).copied().unwrap_or(0);
    Some(CpuSample {
        busy: total.saturating_sub(idle),
        total,
    })
}

fn parse_cpu_cores(s: &str) -> u32 {
    s.lines()
        .filter(|line| {
            let Some(rest) = line.strip_prefix("cpu") else {
                return false;
            };
            !rest.is_empty() && rest.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        })
        .count() as u32
}

/// 解析 `/proc/meminfo` → (MemTotal, MemAvailable, SwapTotal, SwapFree) 字节。
fn parse_meminfo(s: &str) -> (u64, u64, u64, u64) {
    let kb = |key: &str| -> u64 {
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v * 1024)
            .unwrap_or(0)
    };
    (
        kb("MemTotal:"),
        kb("MemAvailable:"),
        kb("SwapTotal:"),
        kb("SwapFree:"),
    )
}

/// 解析 `/proc/net/dev` → 各非 lo 接口 rx/tx 字节累加。
fn parse_net(s: &str) -> Option<NetSample> {
    let mut rx = 0u64;
    let mut tx = 0u64;
    let mut seen = false;
    for line in s.lines() {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();
        if iface == "lo" || iface.is_empty() {
            continue;
        }
        let f: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|x| x.parse().ok())
            .collect();
        // rx bytes = f[0],tx bytes = f[8]。
        if f.len() >= 9 {
            rx += f[0];
            tx += f[8];
            seen = true;
        }
    }
    seen.then_some(NetSample { rx, tx })
}

fn parse_interfaces(
    s: &str,
    previous: &mut HashMap<String, NetInterfaceSample>,
    elapsed: f64,
) -> Vec<NetInterfaceStats> {
    let mut names = Vec::new();
    let mut interfaces = Vec::new();
    for line in s.lines() {
        let Some((name, fields)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let values: Vec<u64> = fields
            .split_whitespace()
            .filter_map(|value| value.parse().ok())
            .collect();
        if values.len() < 9 {
            continue;
        }
        let current = NetInterfaceSample {
            rx: values[0],
            tx: values[8],
        };
        let (rx_rate, tx_rate) = previous
            .get(name)
            .filter(|_| elapsed > 0.0)
            .map(|prior| {
                (
                    (current.rx.saturating_sub(prior.rx) as f64 / elapsed) as u64,
                    (current.tx.saturating_sub(prior.tx) as f64 / elapsed) as u64,
                )
            })
            .unwrap_or((0, 0));
        previous.insert(name.to_string(), current);
        names.push(name.to_string());
        interfaces.push(NetInterfaceStats {
            name: name.to_string(),
            rx_rate,
            tx_rate,
            is_virtual: is_virtual_interface(name),
        });
    }
    previous.retain(|name, _| names.contains(name));
    interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    interfaces
}

fn is_virtual_interface(name: &str) -> bool {
    name == "lo"
        || name.starts_with("veth")
        || name.starts_with("br-")
        || name.starts_with("docker")
        || name.starts_with("virbr")
        || name.starts_with("cni")
        || name.starts_with("flannel")
        || name.starts_with("tun")
        || name.starts_with("tap")
}

fn parse_system_info(s: &str) -> SystemInfo {
    let mut info = SystemInfo::default();
    for line in s.lines() {
        if let Some(value) = line.strip_prefix("hostname=") {
            info.hostname = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("distro=") {
            let value = value.trim();
            if !value.is_empty() {
                info.distro = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("arch=") {
            let value = value.trim();
            if !value.is_empty() {
                info.architecture = Some(value.to_string());
            }
        }
    }
    info
}

/// 解析 `df -P -k` → 各真实挂载点使用情况(跳过伪文件系统)。
fn parse_df(s: &str) -> Vec<DiskUsage> {
    let mut out = Vec::new();
    for line in s.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        // Filesystem 1024-blocks Used Available Capacity Mounted-on
        if f.len() < 6 {
            continue;
        }
        let mount = f[5].to_string();
        // 只保留以 / 开头的挂载点。Keep real `/`-rooted mounts.
        if !mount.starts_with('/') {
            continue;
        }
        let total = f[1].parse::<u64>().unwrap_or(0) * 1024;
        let used = f[2].parse::<u64>().unwrap_or(0) * 1024;
        if total == 0 {
            continue;
        }
        out.push(DiskUsage { mount, used, total });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_delta_percent() {
        let mut prev = Some(CpuSample {
            busy: 100,
            total: 200,
        });
        let mut net = None;
        let mut cores = HashMap::new();
        let mut interfaces = HashMap::new();
        let block = format!(
            "x{BEGIN}\ncpu  150 0 0 250 0 0 0 0\ncpu0 75 0 0 125 0 0 0 0\n{SEC}\n{SEC}\n{SEC}\n{SEC}\n{SEC}\n{SEC}\n{END}y"
        );
        // busy=150,total=400;delta busy=50,total=200 → 25%
        let s = parse_block(
            &block,
            &mut prev,
            &mut net,
            &mut cores,
            &mut interfaces,
            2.0,
        )
        .unwrap();
        assert!((s.cpu_percent - 25.0).abs() < 0.01, "{}", s.cpu_percent);
        assert_eq!(s.cpu_cores, 1);
    }

    #[test]
    fn cpu_cores_count_only_per_core_lines() {
        let s = "cpu  1 0 0 2\ncpu0 1 0 0 1\ncpu1 0 0 0 1\nctxt 10\n";
        assert_eq!(parse_cpu_cores(s), 2);
    }

    #[test]
    fn meminfo_parsed() {
        let (t, a, _st, _sf) = parse_meminfo("MemTotal:       1024 kB\nMemAvailable:    512 kB\n");
        assert_eq!(t, 1024 * 1024);
        assert_eq!(a, 512 * 1024);
    }

    #[test]
    fn df_skips_pseudo() {
        let s = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                 /dev/sda1 1000 400 500 40% /\n\
                 tmpfs 100 0 100 0% /dev/shm\n\
                 udev 50 0 50 0% notapath\n";
        let d = parse_df(s);
        assert_eq!(d.len(), 2); // / 与 /dev/shm(均以 / 开头)
        assert_eq!(d[0].mount, "/");
        assert_eq!(d[0].used, 400 * 1024);
        assert_eq!(d[0].total, 1000 * 1024);
    }

    #[test]
    fn malformed_df_section_does_not_break_other_metrics() {
        let mut prev_cpu = None;
        let mut prev_net = None;
        let mut prev_cores = HashMap::new();
        let mut prev_interfaces = HashMap::new();
        let block = format!(
            "{BEGIN}\ncpu  1 0 0 3\ncpu0 1 0 0 3\n{SEC}\nMemTotal: 1024 kB\nMemAvailable: 512 kB\n{SEC}\n{SEC}\ninvalid df output\n{SEC}\n0.25 0.10 0.05\n{SEC}\n3600.0 0.0\n{SEC}\n{END}"
        );

        let stats = parse_block(
            &block,
            &mut prev_cpu,
            &mut prev_net,
            &mut prev_cores,
            &mut prev_interfaces,
            2.0,
        )
        .unwrap();

        assert_eq!(stats.mem_used, 512 * 1024);
        assert_eq!(stats.load1, 0.25);
        assert_eq!(stats.uptime_secs, 3600);
        assert!(stats.disks.is_empty());
    }

    #[test]
    fn net_excludes_lo() {
        let s = "Inter-|   Receive\n face |bytes ...\n\
                 lo: 999 0 0 0 0 0 0 0 999 0 0 0 0 0 0 0\n\
                 eth0: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\n";
        let n = parse_net(s).unwrap();
        assert_eq!(n.rx, 100);
        assert_eq!(n.tx, 200);
    }

    #[test]
    fn per_core_and_interface_rates_use_deltas() {
        let mut cores = HashMap::from([(
            0,
            CpuSample {
                busy: 10,
                total: 20,
            },
        )]);
        let mut interfaces =
            HashMap::from([("eth0".to_string(), NetInterfaceSample { rx: 100, tx: 200 })]);
        let cpu = "cpu 20 0 0 20\ncpu0 15 0 0 25";
        let net =
            "eth0: 300 0 0 0 0 0 0 0 500 0 0 0 0 0 0 0\nveth1: 20 0 0 0 0 0 0 0 30 0 0 0 0 0 0 0";

        let core = parse_cpu_core_usage(cpu, &mut cores);
        let interface = parse_interfaces(net, &mut interfaces, 2.0);

        assert_eq!(core.len(), 1);
        assert!((core[0].percent - 25.0).abs() < 0.01);
        assert_eq!(interface[0].name, "eth0");
        assert_eq!(interface[0].rx_rate, 100);
        assert_eq!(interface[0].tx_rate, 150);
        assert!(interface[1].is_virtual);
    }

    #[test]
    fn system_info_is_typed_and_empty_values_are_ignored() {
        let info = parse_system_info("hostname=node-1\ndistro=Debian GNU/Linux\narch=x86_64\n");
        assert_eq!(info.hostname, "node-1");
        assert_eq!(info.distro.as_deref(), Some("Debian GNU/Linux"));
        assert_eq!(info.architecture.as_deref(), Some("x86_64"));
    }

    #[test]
    fn system_info_fields_keep_separate_lines_when_commands_are_empty() {
        let info = parse_system_info("hostname=\narch=x86_64\n");
        assert!(info.hostname.is_empty());
        assert_eq!(info.architecture.as_deref(), Some("x86_64"));
        assert!(CMD.contains("printf 'hostname=%s\\n'"));
        assert!(CMD.contains("printf 'arch=%s\\n'"));
    }

    #[test]
    fn heartbeat_command_stays_lightweight_and_separate_from_resource_sample() {
        assert!(HEARTBEAT_CMD.contains("printf"));
        assert!(HEARTBEAT_CMD.contains(HEARTBEAT));
        assert!(!HEARTBEAT_CMD.contains("/proc"));
        assert!(!HEARTBEAT_CMD.contains("ps "));
        assert!(!CMD.contains(HEARTBEAT));
    }

    #[test]
    fn latency_probe_target_uses_ssh_host_and_port() {
        let target = LatencyProbeTarget::new("example.com", 2222);

        assert_eq!(target.host, "example.com");
        assert_eq!(target.port, 2222);
    }

    #[tokio::test]
    async fn tcp_latency_probe_reports_open_ssh_port_latency() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let accept = tokio::spawn(async move {
            let _ = listener.accept().await.unwrap();
        });

        let target = LatencyProbeTarget::new("127.0.0.1", port);
        let latency = measure_tcp_connect_latency(&target).await.unwrap();

        assert!(latency >= 1);
        accept.await.unwrap();
    }

    #[tokio::test]
    async fn tcp_latency_probe_failure_can_fall_back_without_sample_error() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let target = LatencyProbeTarget::new("127.0.0.1", port);

        assert_eq!(measure_tcp_connect_latency(&target).await, None);
    }
}
