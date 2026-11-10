import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import {
  useLayoutEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import {
  Backend,
  errorText,
  useApp,
  type Profile,
  type SessionSnapshot,
} from "../lib/backend";

import { useText } from "../lib/useText";
import { CredentialField } from "./CredentialField";
import { useRestartProtection } from "../lib/useProtectedState";
export function Modal({
  open,
  title,
  description,
  onOpenChange,
  children,
  protectInstall = true,
}: {
  open: boolean;
  title: string;
  description: string;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
  protectInstall?: boolean;
}) {
  const t = useText();
  const installing = useApp((state) => state.installing);
  const content = useRef<HTMLDivElement>(null);
  useRestartProtection(open && protectInstall);
  useLayoutEffect(() => {
    if (content.current) content.current.inert = installing;
  }, [installing, children]);
  return (
    <Dialog.Root
      open={open}
      onOpenChange={(value) => {
        if (!useApp.getState().installing) onOpenChange(value);
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay" />
        <Dialog.Content
          ref={content}
          className="dialog-content"
          onEscapeKeyDown={(event) => {
            if (useApp.getState().installing) event.preventDefault();
          }}
        >
          <header>
            <Dialog.Title>{title}</Dialog.Title>
            <Dialog.Close className="icon-button" aria-label={t("关闭")}>
              <X size={18} />
            </Dialog.Close>
          </header>
          <Dialog.Description className="muted">
            {description}
          </Dialog.Description>
          {children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
export const blankProfile = (): Profile => ({
  name: "",
  host: "",
  user: "",
  port: 22,
  auth: [{ kind: "password" }],
  proxy: { kind: "direct" },
  forward_agent: false,
});
export function ProfileDialog({
  profile,
  onClose,
}: {
  profile: Profile;
  onClose: () => void;
}) {
  const t = useText();
  const [draft, setDraft] = useState(profile);
  const [password, setPassword] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [savedName, setSavedName] = useState<string | null>(null);
  const update = (field: keyof Profile, value: string | number) =>
    setDraft((state) => ({ ...state, [field]: value }));
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (saving) return;
    setSaving(true);
    setError("");
    try {
      const snapshot = useApp.getState().config;
      if (!snapshot) throw new Error(t("配置尚未加载"));
      if (!savedName) {
        const next = await Backend.SaveProfile({
          expectedRevision: snapshot.revision,
          originalName: profile.name,
          profile: draft,
        });
        useApp.getState().apply(next);
        setSavedName(draft.name);
      }
      if (password) await Backend.SaveSecret(savedName ?? draft.name, password);
      setPassword("");
      onClose();
    } catch (error) {
      setError(errorText(error));
    } finally {
      setSaving(false);
    }
  }
  const auth = draft.auth?.[0] ?? { kind: "password" };
  return (
    <Modal
      open
      title={t(profile.name ? "编辑连接" : "新建连接")}
      description={t("密码单独加密保存；连接配置不包含密码明文。")}
      onOpenChange={(open) => {
        if (!open && !saving) onClose();
      }}
    >
      <form onSubmit={submit} className="form-grid">
        <fieldset disabled={saving || savedName !== null}>
          <label>
            {t("名称")}
            <input
              required
              maxLength={256}
              value={draft.name}
              onChange={(e) => update("name", e.target.value)}
            />
          </label>
          <label>
            {t("分组")}
            <input
              value={draft.group ?? ""}
              onChange={(e) => update("group", e.target.value)}
            />
          </label>
          <div className="form-row">
            <label>
              {t("主机")}
              <input
                required
                autoCapitalize="none"
                autoCorrect="off"
                value={draft.host}
                onChange={(e) => update("host", e.target.value)}
              />
            </label>
            <label className="port">
              {t("端口")}
              <input
                type="number"
                min={1}
                max={65535}
                value={draft.port}
                onChange={(e) => update("port", Number(e.target.value))}
              />
            </label>
          </div>
          <label>
            {t("用户名")}
            <input
              required
              autoCapitalize="none"
              autoCorrect="off"
              value={draft.user}
              onChange={(e) => update("user", e.target.value)}
            />
          </label>
          <label>
            {t("认证方式")}
            <select
              value={auth.kind}
              onChange={(e) =>
                setDraft((s) => ({
                  ...s,
                  auth: [{ ...auth, kind: e.target.value }],
                }))
              }
            >
              <option value="password">{t("密码")}</option>
              <option value="public_key">{t("私钥")}</option>
              <option value="keyboard_interactive">{t("交互认证")}</option>
              <option value="agent">SSH agent</option>
            </select>
          </label>
          {auth.kind === "public_key" && (
            <label>
              {t("私钥路径")}
              <input
                required
                value={auth.key_path ?? ""}
                onChange={(e) =>
                  setDraft((s) => ({
                    ...s,
                    auth: [{ ...auth, key_path: e.target.value }],
                  }))
                }
              />
            </label>
          )}
          <label>
            {t("单跳 ProxyJump")}
            <input
              placeholder="user@host:22"
              value={draft.proxy_jump ?? ""}
              onChange={(e) => update("proxy_jump", e.target.value)}
            />
          </label>
          <label>
            {t("TCP 代理")}
            <select
              value={draft.proxy.kind}
              onChange={(e) =>
                setDraft((s) => ({
                  ...s,
                  proxy: { ...s.proxy, kind: e.target.value },
                }))
              }
            >
              <option value="direct">{t("直连")}</option>
              <option value="system">{t("环境代理")}</option>
              <option value="socks5">SOCKS5</option>
              <option value="http">HTTP CONNECT</option>
            </select>
          </label>
          {["socks5", "http"].includes(draft.proxy.kind) && (
            <div className="form-row">
              <label>
                {t("代理主机")}
                <input
                  required
                  value={draft.proxy.host ?? ""}
                  onChange={(e) =>
                    setDraft((s) => ({
                      ...s,
                      proxy: { ...s.proxy, host: e.target.value },
                    }))
                  }
                />
              </label>
              <label className="port">
                {t("端口")}
                <input
                  required
                  type="number"
                  min={1}
                  max={65535}
                  value={draft.proxy.port ?? 1080}
                  onChange={(e) =>
                    setDraft((s) => ({
                      ...s,
                      proxy: { ...s.proxy, port: Number(e.target.value) },
                    }))
                  }
                />
              </label>
            </div>
          )}
        </fieldset>
        <CredentialField
          target={
            !savedName && JSON.stringify(profile) === JSON.stringify(draft)
              ? profile
              : null
          }
          label={t(auth.kind === "public_key" ? "保存私钥口令" : "保存密码")}
          value={password}
          disabled={saving}
          onChange={setPassword}
        />
        {savedName && error && (
          <p className="notice">
            {t("连接配置已保存；凭据保存失败，可重试。")}
          </p>
        )}
        {error && (
          <p role="alert" className="error">
            {error}
          </p>
        )}
        <footer>
          <button type="button" onClick={onClose} disabled={saving}>
            {t("取消")}
          </button>
          <button className="primary" disabled={saving}>
            {t(saving ? "保存中…" : "保存")}
          </button>
        </footer>
      </form>
    </Modal>
  );
}
export function ChallengeDialog({ session }: { session: SessionSnapshot }) {
  const t = useText();
  const challenge = session.challenge!;
  const [answers, setAnswers] = useState<string[]>(() =>
    (challenge.prompts ?? []).map(() => ""),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function answer(trust: boolean, cancel = false) {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await Backend.AnswerChallenge({
        sessionId: session.id,
        generation: session.generation,
        challengeId: challenge.id,
        answers: cancel ? [] : answers,
        trust,
      });
    } catch (error) {
      setError(errorText(error));
      setBusy(false);
    }
  }
  return (
    <Modal
      open
      title={t(challenge.kind === "host_key" ? "核对主机密钥" : "连接认证")}
      description={`${challenge.host}:${challenge.port} ${challenge.message}`}
      onOpenChange={(open) => {
        if (!open) void answer(false, true);
      }}
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void answer(challenge.kind === "host_key");
        }}
        className="form-grid"
      >
        {challenge.kind === "host_key" && (
          <>
            <p>{t("请与服务器管理员提供的指纹核对，确认后写入本机信任库。")}</p>
            <code className="fingerprint">{challenge.fingerprint}</code>
          </>
        )}
        {(challenge.prompts ?? []).map((prompt, index) => (
          <label key={index}>
            {prompt.text}
            <input
              autoFocus={index === 0}
              type={prompt.echo ? "text" : "password"}
              value={answers[index]}
              autoComplete="off"
              onChange={(e) =>
                setAnswers((current) =>
                  current.map((value, i) =>
                    i === index ? e.target.value : value,
                  ),
                )
              }
            />
          </label>
        ))}
        {error && (
          <p role="alert" className="error">
            {error}
          </p>
        )}
        <footer>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              void answer(false, true);
            }}
          >
            {t("取消连接")}
          </button>
          <button className="primary" disabled={busy}>
            {t(
              busy
                ? "提交中…"
                : challenge.kind === "host_key"
                  ? "指纹一致，信任"
                  : "提交",
            )}
          </button>
        </footer>
      </form>
    </Modal>
  );
}
