//! 服务器资源监控子任务 —— 通过一条持久 `sh` 通道周期采集远端 `/proc` 等数据。
//!
//! Server resource monitor: drives a persistent `sh` channel, periodically writing
//! a command bundle and parsing the output into [`MonitorStats`]. CPU% and network
//! rates are computed from deltas between polls. Linux-only (`/proc`); missing
//! fields degrade gracefully. Runs in its own task so it never blocks the shell.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::session::{FromCore, SessionId};

/// 轮询间隔。Poll interval.
const POLL: std::time::Duration = std::time::Duration::from_secs(2);
const SAMPLE_TIMEOUT: Duration = Duration::from_secs(12);

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
ps -eo pcpu,pmem,comm --sort=-pcpu 2>/dev/null | head -n 9;\
echo __KTM_END__\n";

const BEGIN: &str = "__KTM_BEGIN__";
const SEC: &str = "__KTM_SEC__";
const END: &str = "__KTM_END__";

/// 单个挂载点使用情况。Disk usage for one mount point.
#[derive(Debug, Clone)]
pub struct DiskUsage {
    pub mount: String,
    pub used: u64,
    pub total: u64,
}

/// 单个进程占用。One process's usage.
#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub cpu: f32,
    pub mem: f32,
    pub name: String,
}

/// 一次采样的服务器资源快照。A snapshot of server resource usage.
#[derive(Debug, Clone, Default)]
pub struct MonitorStats {
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
    /// 一次监控命令从写入到读完整块的耗时，近似远端监控通道延迟。
    pub latency_ms: u64,
    pub disks: Vec<DiskUsage>,
    pub processes: Vec<ProcInfo>,
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

/// 监控子任务主循环。通道关闭(会话结束)即退出。
/// Monitor loop; exits when the channel closes (session ended).
pub(crate) async fn monitor_task(
    id: SessionId,
    mut channel: russh::Channel<russh::client::Msg>,
    out: mpsc::UnboundedSender<FromCore>,
) -> MonitorExit {
    let mut prev_cpu: Option<CpuSample> = None;
    let mut prev_net: Option<NetSample> = None;
    let mut prev_at: Option<Instant> = None;
    let mut buf = String::new();

    let exit = loop {
        let sample_started = Instant::now();
        // 写入命令包。Write the command bundle.
        if channel.data(CMD.as_bytes()).await.is_err() {
            let _ = out.send(FromCore::MonitorError {
                id,
                message: "资源监控命令发送失败".to_string(),
            });
            break MonitorExit::ErrorReported;
        }

        // 读取到 END 哨兵为止。Read until the END sentinel.
        buf.clear();
        let mut closed = false;
        let mut failure_message = None;
        let sample_timeout = tokio::time::sleep(SAMPLE_TIMEOUT);
        tokio::pin!(sample_timeout);
        loop {
            tokio::select! {
                _ = &mut sample_timeout => {
                    failure_message = Some(format!(
                        "资源监控采样超时({} 秒)",
                        SAMPLE_TIMEOUT.as_secs()
                    ));
                    closed = true;
                    break;
                }
                msg = channel.wait() => {
                    match msg {
                        Some(russh::ChannelMsg::Data { data }) => {
                            buf.push_str(&String::from_utf8_lossy(&data));
                            if buf.contains(END) {
                                break;
                            }
                        }
                        Some(russh::ChannelMsg::ExtendedData { .. }) => {}
                        Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => {
                            closed = true;
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
        if closed {
            if let Some(message) = failure_message {
                let _ = out.send(FromCore::MonitorError { id, message });
                break MonitorExit::ErrorReported;
            }
            break MonitorExit::Stopped;
        }

        let now = Instant::now();
        let elapsed = prev_at
            .map(|p| now.duration_since(p).as_secs_f64())
            .unwrap_or(0.0);
        prev_at = Some(now);

        if let Some(mut stats) = parse_block(&buf, &mut prev_cpu, &mut prev_net, elapsed) {
            stats.latency_ms = sample_started.elapsed().as_millis() as u64;
            if out
                .send(FromCore::Monitor {
                    id,
                    stats: Box::new(stats),
                })
                .is_err()
            {
                break MonitorExit::ReceiverDropped;
            }
        } else {
            let _ = out.send(FromCore::MonitorError {
                id,
                message: "资源监控采样解析失败".to_string(),
            });
            break MonitorExit::ErrorReported;
        }

        tokio::time::sleep(POLL).await;
    };

    // 关闭 sh:写 EOF 让远端进程退出。
    // Close sh by sending EOF so the remote process exits.
    let _ = channel.eof().await;
    exit
}

/// 解析一次输出块。`prev_*` 在内部更新以便下次算增量。
/// Parse one output block; updates `prev_*` for next-poll deltas.
fn parse_block(
    raw: &str,
    prev_cpu: &mut Option<CpuSample>,
    prev_net: &mut Option<NetSample>,
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

    // --- PROCESSES ---
    stats.processes = parse_ps(get(6));

    Some(stats)
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
        let used = f[2].parse::<u64>().unwrap_or(0) * 1024;
        let avail = f[3].parse::<u64>().unwrap_or(0) * 1024;
        let total = used + avail;
        if total == 0 {
            continue;
        }
        out.push(DiskUsage { mount, used, total });
    }
    out
}

/// 解析 `ps` 输出 → Top 进程(跳过表头)。
fn parse_ps(s: &str) -> Vec<ProcInfo> {
    let mut out = Vec::new();
    for line in s.lines().skip(1) {
        let mut it = line.split_whitespace();
        let (Some(cpu), Some(mem)) = (it.next(), it.next()) else {
            continue;
        };
        let name: String = it.collect::<Vec<_>>().join(" ");
        if name.is_empty() {
            continue;
        }
        out.push(ProcInfo {
            cpu: cpu.parse().unwrap_or(0.0),
            mem: mem.parse().unwrap_or(0.0),
            name,
        });
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
        let block = format!(
            "x{BEGIN}\ncpu  150 0 0 250 0 0 0 0\ncpu0 75 0 0 125 0 0 0 0\n{SEC}\n{SEC}\n{SEC}\n{SEC}\n{SEC}\n{SEC}\n{END}y"
        );
        // busy=150,total=400;delta busy=50,total=200 → 25%
        let s = parse_block(&block, &mut prev, &mut net, 2.0).unwrap();
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
                 /dev/sda1 1000 400 600 40% /\n\
                 tmpfs 100 0 100 0% /dev/shm\n\
                 udev 50 0 50 0% notapath\n";
        let d = parse_df(s);
        assert_eq!(d.len(), 2); // / 与 /dev/shm(均以 / 开头)
        assert_eq!(d[0].mount, "/");
        assert_eq!(d[0].used, 400 * 1024);
        assert_eq!(d[0].total, 1000 * 1024);
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
}
