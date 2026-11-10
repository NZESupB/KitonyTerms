import { useEffect, useRef, useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import {
  Backend,
  useApp,
  type Profile,
  type SecretState,
} from "../lib/backend";
import { SecretView, type SecretViewState } from "../lib/secret-view";
import { useText } from "../lib/useText";
import type { TextKey } from "../lib/i18n";

const failures: Record<string, TextKey> = {
  cancelled: "系统认证已取消",
  unavailable: "此平台无法查看已保存的凭据",
  busy: "已有系统认证正在进行",
  wrong_credentials: "账户密码错误或与当前用户不匹配",
  restricted: "系统限制了该账户的登录",
  locked: "系统账户已锁定",
  target_changed: "连接配置已改变，请重新打开连接设置",
  missing: "未保存该连接的凭据",
  failed: "系统认证失败",
};

export function CredentialField({
  target,
  label,
  value,
  disabled,
  onChange,
}: {
  target: Profile | null;
  label: string;
  value: string;
  disabled: boolean;
  onChange: (value: string) => void;
}) {
  const t = useText();
  const config = useApp((state) => state.config);
  const quitting = useApp((state) => state.quitting);
  const current = config?.config.sessions?.find(
    (profile) => profile.name === target?.name,
  );
  const owner =
    target && JSON.stringify(current) === JSON.stringify(target)
      ? JSON.stringify(target)
      : "";
  const mounted = useRef(false);
  const [snapshot, setSnapshot] = useState<SecretViewState>({
    owner: "",
    value: null,
    busy: false,
    failure: "",
  });
  const [view] = useState(
    () =>
      new SecretView((state) => {
        if (mounted.current) setSnapshot(state);
      }),
  );
  const [presence, setPresence] = useState<{
    owner: string;
    state: SecretState;
  } | null>(null);
  const [presenceFailure, setPresenceFailure] = useState(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      view.clear();
    };
  }, [view]);
  useEffect(() => {
    view.own(owner);
    setPresence(null);
    setPresenceFailure(false);
    if (!owner || !target) return;
    let stopped = false;
    const request = Backend.GetSecretState(target);
    void request.then(
      (state) => {
        if (!stopped) setPresence({ owner, state });
      },
      () => {
        if (!stopped) setPresenceFailure(true);
      },
    );
    return () => {
      stopped = true;
      void request.cancel().catch(() => {});
    };
  }, [owner, target, view]);
  useEffect(() => {
    if (disabled || quitting || value) view.clear();
  }, [disabled, quitting, value, view]);
  useEffect(() => {
    const hidden = () => {
      if (document.visibilityState !== "visible") view.clear();
    };
    document.addEventListener("visibilitychange", hidden);
    return () => document.removeEventListener("visibilitychange", hidden);
  }, [view]);
  const allowed = !!owner && !disabled && !quitting && !value;
  const state = presence?.owner === owner ? presence.state : null;
  const shown = allowed && snapshot.owner === owner ? snapshot.value : null;
  const busy = allowed && snapshot.owner === owner && snapshot.busy;
  const failure = allowed && snapshot.owner === owner ? snapshot.failure : "";
  function reveal(method: string) {
    if (
      !allowed ||
      !state?.present ||
      !state.available ||
      !target ||
      document.visibilityState !== "visible"
    )
      return;
    view.own(owner);
    void view.reveal(() => Backend.RevealSecret(target, method));
  }
  return (
    <div className="credential-field">
      <label>
        {label}
        <input
          type="password"
          autoComplete="new-password"
          value={value}
          disabled={disabled}
          placeholder={t("留空则不更改")}
          onChange={(event) => {
            view.clear();
            onChange(event.target.value);
          }}
        />
      </label>
      {state?.present && (
        <div className="credential-actions">
          {state.available ? (
            <>
              <button
                type="button"
                disabled={!allowed || busy}
                onClick={() => (shown === null ? reveal("auto") : view.clear())}
              >
                {shown === null ? <Eye size={14} /> : <EyeOff size={14} />}
                {t(shown === null ? "查看已保存的凭据" : "隐藏凭据")}
              </button>
              {state.accountPassword && (
                <button
                  type="button"
                  disabled={!allowed || busy}
                  onClick={() => reveal("account_password")}
                >
                  {t("使用账户密码验证")}
                </button>
              )}
              {busy && (
                <button type="button" onClick={() => view.clear()}>
                  {t("取消验证")}
                </button>
              )}
            </>
          ) : (
            <span className="muted">{t("此平台无法查看已保存的凭据")}</span>
          )}
        </div>
      )}
      {busy && (
        <p className="muted" role="status">
          {t("等待系统身份验证…")}
        </p>
      )}
      {shown !== null && (
        <label>
          {t("已保存的凭据（只读）")}
          <input
            value={shown}
            readOnly
            type="text"
            autoComplete="off"
            autoCapitalize="none"
            spellCheck={false}
          />
          <span className="muted">
            {t("仅供查看，30秒后隐藏；不会作为新密码保存。")}
          </span>
        </label>
      )}
      {owner && presenceFailure && (
        <p className="error" role="alert">
          {t("无法检查已保存的凭据，请重新打开连接设置。")}
        </p>
      )}
      {failure && (
        <p
          className={failure === "cancelled" ? "muted" : "error"}
          role={failure === "cancelled" ? "status" : "alert"}
        >
          {t(failures[failure] ?? "系统认证失败")}
        </p>
      )}
    </div>
  );
}
