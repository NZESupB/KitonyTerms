import { useEffect, useId, useRef, useState } from "react";
import { Clock3, Copy, RotateCw, Unplug } from "lucide-react";
import type { SessionSnapshot } from "../lib/backend";
import { writeClipboard } from "../lib/clipboard";
import type { TextKey } from "../lib/i18n";
import { useText } from "../lib/useText";

const statusLabel: Record<string, TextKey> = {
  connecting: "连接中",
  closing: "关闭中",
  closed: "连接已断开",
};

type CopyStatus = "copying" | "copied" | "error";

type ConnectionStatusProps = {
  session: SessionSnapshot;
  profileAvailable: boolean;
  busy: boolean;
  reconnectError: string;
  onReconnect: () => void;
  onOpenConnections: () => void;
};

export function ConnectionStatus({
  session,
  profileAvailable,
  busy,
  reconnectError,
  onReconnect,
  onOpenConnections,
}: ConnectionStatusProps) {
  const t = useText();
  const hintId = useId();
  const copyVersion = useRef(0);
  const [copy, setCopy] = useState<{ text: string; status: CopyStatus } | null>(
    null,
  );
  useEffect(
    () => () => {
      copyVersion.current++;
    },
    [],
  );
  const closed = session.status === "closed";
  const StatusIcon = closed ? Unplug : Clock3;
  const detailsText = [
    session.error && `${t("断开原因")}\n${session.error}`,
    reconnectError && `${t("重新连接失败")}\n${reconnectError}`,
  ]
    .filter(Boolean)
    .join("\n\n");
  const copyStatus = copy?.text === detailsText ? copy.status : null;

  async function copyDetails() {
    const version = ++copyVersion.current;
    setCopy({ text: detailsText, status: "copying" });
    try {
      await writeClipboard(detailsText);
      if (version === copyVersion.current)
        setCopy({ text: detailsText, status: "copied" });
    } catch {
      if (version === copyVersion.current)
        setCopy({ text: detailsText, status: "error" });
    }
  }

  if (session.status === "connected") return null;

  return (
    <section className="connection-status" aria-label={t("会话状态")}>
      <div className="connection-status-row">
        <p className="connection-status-title" role="status" aria-atomic="true">
          <StatusIcon size={16} aria-hidden="true" />
          <strong>
            {statusLabel[session.status]
              ? t(statusLabel[session.status])
              : session.status}
          </strong>
        </p>
        {closed &&
          (profileAvailable ? (
            <button
              type="button"
              disabled={busy}
              aria-describedby={hintId}
              title={`${t("将在新标签页重新连接，旧会话及未保存内容将保留。")} ${t("将建立新的 SSH 会话，不会恢复原来的 Shell 或进程。")}`}
              onClick={onReconnect}
            >
              <RotateCw size={14} aria-hidden="true" />
              {t("重新连接")}
            </button>
          ) : (
            <button
              type="button"
              aria-describedby={hintId}
              onClick={onOpenConnections}
            >
              {t("打开连接管理")}
            </button>
          ))}
      </div>
      {closed && (
        <p className="connection-status-hint" id={hintId}>
          {profileAvailable
            ? t("将在新标签页重新连接。")
            : t("此会话的连接配置已删除或改名，请在连接管理中选择或新建连接。")}
        </p>
      )}
      {reconnectError && (
        <p className="connection-status-failure" role="status">
          {t("重新连接失败，请重试。")}
        </p>
      )}
      {detailsText && (
        <details className="connection-status-details">
          <summary
            onMouseDown={(event) => {
              // 鼠标展开时保留终端焦点；键盘仍使用原生 summary 行为。
              if (event.button === 0) event.preventDefault();
            }}
          >
            {t("查看详情")}
          </summary>
          <pre tabIndex={0} aria-label={t("连接错误详情")}>
            {detailsText}
          </pre>
          <div className="connection-status-copy">
            <button
              type="button"
              disabled={copyStatus === "copying"}
              onClick={() => void copyDetails()}
            >
              <Copy size={14} aria-hidden="true" />
              {t("复制详情")}
            </button>
            <span role="status">
              {copyStatus === "copied"
                ? t("已复制详情")
                : copyStatus === "error"
                  ? t("复制失败，请选择详情手动复制。")
                  : ""}
            </span>
          </div>
        </details>
      )}
    </section>
  );
}
