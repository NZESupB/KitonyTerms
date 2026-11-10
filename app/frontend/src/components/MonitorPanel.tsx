import { useEffect, useState, type CSSProperties } from "react";
import {
  Activity,
  Cpu,
  HardDrive,
  MemoryStick,
  Network,
  type LucideIcon,
} from "lucide-react";
import {
  Backend,
  useApp,
  type MonitorSnapshot,
  type SessionSnapshot,
} from "../lib/backend";
import { formatBytes } from "../lib/transfers";
import {
  clampPercent,
  latencyLevel,
  metricPercent,
  metricRate,
  metricUptime,
  networkActivity,
} from "../lib/monitor";

import { useText } from "../lib/useText";
type LocalSnapshot = {
  owner: string;
  value: MonitorSnapshot | null;
  failed: boolean;
};

function useMonitor(session: SessionSnapshot) {
  const owner = `${session.id}:${session.generation}`;
  const quitting = useApp((state) => state.quitting);
  const [latest, setLatest] = useState<LocalSnapshot | null>(null);
  useEffect(() => {
    if (session.status !== "connected" || quitting) return;
    let disposed = false;
    let stopCycle: (() => void) | undefined;
    const start = () => {
      if (disposed || document.visibilityState !== "visible") return;
      let stopped = false;
      const watchID = crypto.randomUUID().replace(/-/g, "");
      let timer: ReturnType<typeof setTimeout>;
      let query: ReturnType<typeof Backend.PollMonitor> | undefined;
      const release = () =>
        void Backend.StopMonitor(session.id, session.generation, watchID).catch(
          () => {},
        );
      const poll = async () => {
        if (stopped) return;
        try {
          query = Backend.PollMonitor(session.id, session.generation, watchID);
          const value = await query;
          if (
            !stopped &&
            value.sessionId === session.id &&
            value.generation === session.generation
          )
            setLatest({ owner, value, failed: false });
        } catch {
          if (!stopped)
            setLatest((prior) => ({
              owner,
              value: prior?.owner === owner ? prior.value : null,
              failed: true,
            }));
        } finally {
          query = undefined;
          if (stopped) release();
          else timer = setTimeout(() => void poll(), 1000);
        }
      };
      stopCycle = () => {
        stopped = true;
        clearTimeout(timer);
        if (query) void query.cancel().catch(() => {});
        release();
      };
      void poll();
    };
    const visibility = () => {
      stopCycle?.();
      stopCycle = undefined;
      start();
    };
    document.addEventListener("visibilitychange", visibility);
    start();
    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", visibility);
      stopCycle?.();
    };
  }, [owner, session.id, session.generation, session.status, quitting]);
  return latest?.owner === owner && session.status === "connected"
    ? latest
    : null;
}

function Metric({
  icon: Icon,
  label,
  value,
  subvalue,
  percent,
  tone,
  state = "",
}: {
  icon: LucideIcon;
  label: string;
  value: string;
  subvalue: string;
  percent: number | null | undefined;
  tone: string;
  state?: string;
}) {
  const t = useText();
  return (
    <div
      className={`metric-card ${tone} ${state}`}
      style={
        {
          "--metric-fill-percent": `${clampPercent(percent)}%`,
        } as CSSProperties
      }
    >
      <span className="metric-label" title={label}>
        <Icon size={12} aria-hidden="true" />
        {label}
      </span>
      <div className="metric-value-row" title={`${value} · ${subvalue}`}>
        <strong>{value}</strong>
        <small>{subvalue}</small>
      </div>
      <div
        className="metric-meter"
        role={percent == null ? "img" : "meter"}
        aria-label={t(
          percent == null ? "{name} 使用情况未知" : "{name} 使用情况",
          { name: label },
        )}
        aria-valuemin={percent == null ? undefined : 0}
        aria-valuemax={percent == null ? undefined : 100}
        aria-valuenow={percent == null ? undefined : clampPercent(percent)}
      >
        <div className="metric-fill" />
      </div>
    </div>
  );
}

export function MonitorPanel({ session }: { session: SessionSnapshot }) {
  const t = useText();
  const latest = useMonitor(session);
  const snapshot = latest?.value;
  const stats = snapshot?.stats;
  const failed = latest?.failed || snapshot?.status === "error";
  const waiting = t(
    failed
      ? "暂不可用"
      : session.status === "connected"
        ? "等待采样"
        : "已断开",
  );
  const percent = (value: number | null | undefined, digits = 0) =>
    value == null ? "--" : `${clampPercent(value).toFixed(digits)}%`;
  const memory = stats ? metricPercent(stats.memUsed, stats.memTotal) : null;
  const root = stats?.disks?.find((disk) => disk.mount === "/");
  const disk = root ? metricPercent(root.used, root.total) : null;
  const latency = stats?.latencyMs ?? "";
  return (
    <div
      className={`status-monitor${failed ? " is-stale" : ""}`}
      aria-label={`${t("{name} 资源监控", { name: session.title })}${failed ? ` · ${t("暂不可用")}` : ""}`}
      title={
        failed
          ? `${snapshot?.error || t("监控暂不可用")}${stats ? t("；保留最近成功采样") : ""}`
          : undefined
      }
      data-session-id={session.id}
      data-generation={session.generation}
      data-sequence={snapshot?.sequence ?? "0"}
      data-status={failed ? "error" : (snapshot?.status ?? "loading")}
    >
      <div className="monitor-grid">
        <Metric
          icon={Cpu}
          tone="blue"
          label={failed && stats ? t("CPU · 已过期") : "CPU"}
          value={percent(stats?.cpuPercent, 1)}
          subvalue={
            stats
              ? stats.cpuCores
                ? t("{count} 核心", { count: stats.cpuCores })
                : "--"
              : waiting
          }
          percent={stats?.cpuPercent}
        />
        <Metric
          icon={MemoryStick}
          tone="amber"
          label={t("内存")}
          value={percent(memory)}
          subvalue={
            stats
              ? `${formatBytes(stats.memUsed)} / ${formatBytes(stats.memTotal)}`
              : waiting
          }
          percent={memory}
        />
        <Metric
          icon={HardDrive}
          tone="cyan"
          label={t("磁盘")}
          value={percent(disk)}
          subvalue={
            root
              ? `${formatBytes(root.used)} / ${formatBytes(root.total)}`
              : stats
                ? "--"
                : waiting
          }
          percent={disk}
        />
        <Metric
          icon={Activity}
          tone="green"
          label={t("负载")}
          value={stats?.load1 == null ? "--" : stats.load1.toFixed(2)}
          subvalue={stats ? metricUptime(stats.uptimeSecs) : waiting}
          percent={
            stats?.load1 == null || !stats.cpuCores
              ? null
              : (stats.load1 / stats.cpuCores) * 100
          }
        />
        <Metric
          icon={Network}
          tone="purple"
          state={`latency-${latencyLevel(latency)}`}
          label={`${t("网络")}${latency && latency !== "0" ? ` ${latency}ms` : ""}`}
          value={stats ? `↓ ${metricRate(stats.netRxRate)}` : "--"}
          subvalue={stats ? `↑ ${metricRate(stats.netTxRate)}` : waiting}
          percent={
            stats ? networkActivity(stats.netRxRate, stats.netTxRate) : null
          }
        />
      </div>
    </div>
  );
}
