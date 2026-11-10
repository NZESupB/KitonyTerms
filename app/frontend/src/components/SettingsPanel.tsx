import {
  useEffect,
  useReducer,
  useRef,
  useState,
  type InputHTMLAttributes,
} from "react";
import { ArrowLeft, ChevronRight } from "lucide-react";
import {
  Backend,
  errorText,
  useApp,
  type Editor,
  type Settings,
} from "../lib/backend";
import { SettingsConflict, SettingsWriter } from "../lib/settings";
import { useText } from "../lib/useText";
import type { TextKey } from "../lib/i18n";
import { SyncPanel } from "./SyncPanel";
import { UpdatePanel } from "./UpdatePanel";
import { UpdateDraft } from "../lib/update";
import {
  useProtectedState,
  useRestartProtection,
} from "../lib/useProtectedState";
import { fontOptions } from "../lib/font-options";
import { useFonts } from "../lib/useFonts";

type Category = "general" | "terminal" | "editor" | "sync" | "update";
const categories: { id: Category; label: TextKey }[] = [
  { id: "general", label: "常规" },
  { id: "terminal", label: "终端" },
  { id: "editor", label: "编辑器" },
  { id: "sync", label: "同步" },
  { id: "update", label: "更新" },
];
const phoneQuery = "(max-width: 599px), (max-height: 599px)";

export function SettingsPanel({
  settings,
  guard,
}: {
  settings: Settings;
  guard: { current: () => boolean };
}) {
  const t = useText();
  const info = useApp((state) => state.info);
  const fonts = useFonts();
  const [phone, setPhone] = useState(() => matchMedia(phoneQuery).matches);
  const [category, setCategory] = useState<Category | null>(null);
  const [editors, setEditors] = useState<Editor[]>([]);
  const [editorError, setEditorError] = useState("");
  const [editorName, setEditorName] = useProtectedState("");
  const [editorCommand, setEditorCommand] = useProtectedState("");
  const [updates] = useState(() => new UpdateDraft());
  const [, render] = useReducer((value: number) => value + 1, 0);
  const mounted = useRef(true);
  const [writer] = useState(
    () =>
      new SettingsWriter({
        load: () => Backend.GetConfig(),
        save: (snapshot, value) =>
          Backend.SaveSettings({
            expectedRevision: snapshot.revision,
            settings: value,
          }),
        apply: (snapshot) => useApp.getState().apply(snapshot),
        changed: () => {
          if (mounted.current) render();
        },
      }),
  );
  useEffect(() => {
    mounted.current = true;
    const media = matchMedia(phoneQuery);
    const update = () => setPhone(media.matches);
    media.addEventListener("change", update);
    return () => {
      mounted.current = false;
      media.removeEventListener("change", update);
    };
  }, []);
  const current = category ?? (phone ? null : "general");
  const draft = writer.view(settings);
  guard.current = () =>
    !writer.busy &&
    !updates.saving &&
    ((!writer.dirty && !updates.dirty) ||
      window.confirm(t("关闭会丢弃尚未保存的设置，是否继续？")));
  const canInstall = () =>
    !writer.busy &&
    !writer.dirty &&
    !updates.saving &&
    !updates.dirty &&
    !updates.action &&
    !editorName &&
    !editorCommand;
  useRestartProtection(!canInstall());
  useEffect(() => {
    if (current !== "editor" || phone) return;
    let stopped = false;
    const request = Backend.GetEditors();
    void request.then(
      (value) => {
        if (!stopped) {
          setEditors(value ?? []);
          setEditorError("");
        }
      },
      (error) => {
        if (!stopped) setEditorError(errorText(error));
      },
    );
    return () => {
      stopped = true;
      void request.cancel();
    };
  }, [current, phone, settings.editors, settings.default_editor]);
  const patch = (value: Partial<Settings>) => writer.update(settings, value);
  const available = categories.filter(
    (item) =>
      !(phone && item.id === "editor") &&
      !(info?.mobile && item.id === "update"),
  );
  const check = (
    field:
      | "use_ssh_config"
      | "show_line_numbers"
      | "show_timestamps"
      | "terminal_wrap",
    label: TextKey,
  ) => (
    <label className="checkbox">
      <input
        type="checkbox"
        checked={draft[field]}
        onChange={(event) => patch({ [field]: event.target.checked })}
      />
      {t(label)}
    </label>
  );
  const conflict = writer.error instanceof SettingsConflict;
  function addEditor() {
    const name = editorName.trim(),
      command = editorCommand.trim();
    if (
      !name ||
      !command ||
      (draft.editors ?? []).some((item) => item.command.trim() === command)
    ) {
      setEditorError(t("名称和命令不能为空，命令不能重复。"));
      return;
    }
    patch({ editors: [...(draft.editors ?? []), { name, command }] });
    setEditorName("");
    setEditorCommand("");
    setEditorError("");
  }
  return (
    <section
      className={`settings-panel settings-categories ${phone ? "settings-phone" : ""} ${current ? "has-category" : ""}`}
    >
      <nav className="settings-nav" aria-label={t("设置分类")}>
        {available.map((item) => (
          <button
            key={item.id}
            aria-current={current === item.id ? "page" : undefined}
            onClick={() => setCategory(item.id)}
          >
            {t(item.label)}
            {phone && <ChevronRight size={16} />}
          </button>
        ))}
      </nav>
      {current && (
        <div className="settings-detail">
          <header className="settings-detail-header">
            {phone && (
              <button
                onClick={() => setCategory(null)}
                aria-label={t("返回设置")}
              >
                <ArrowLeft size={18} />
              </button>
            )}
            <h2>{t(categories.find((item) => item.id === current)!.label)}</h2>
          </header>
          <div className="settings-fields">
            {current === "general" && (
              <>
                <label>
                  {t("语言")}
                  <select
                    value={draft.language}
                    onChange={(e) => patch({ language: e.target.value })}
                  >
                    <option value="chinese">简体中文</option>
                    <option value="english">English</option>
                  </select>
                </label>
                <label>
                  {t("主题")}
                  <select
                    value={
                      draft.theme === "dark"
                        ? "default-dark"
                        : draft.theme === "light"
                          ? "default-light"
                          : draft.theme
                    }
                    onChange={(e) => patch({ theme: e.target.value })}
                  >
                    <option value="system">{t("跟随系统")}</option>
                    <option value="default-dark">{t("深色")}</option>
                    <option value="default-light">{t("浅色")}</option>
                  </select>
                </label>
                <label>
                  {t("强调色")}
                  <input
                    type="color"
                    value={draft.accent_color}
                    onChange={(e) => patch({ accent_color: e.target.value })}
                  />
                </label>
                <label>
                  {t("界面字体")}
                  <select
                    value={draft.ui_font_family ?? ""}
                    disabled={fonts.loading}
                    onChange={(e) => patch({ ui_font_family: e.target.value })}
                  >
                    <option value="">{t("系统默认")}</option>
                    {fontOptions(
                      draft.ui_font_family ?? "",
                      fonts.catalog?.families ?? [],
                    ).map((font) => (
                      <option key={font} value={font}>
                        {font}
                      </option>
                    ))}
                  </select>
                </label>
                <FontStatus fonts={fonts} />
                <label>
                  {t("界面字号缩放")}
                  <select
                    value={Math.round(draft.ui_font_scale * 100)}
                    onChange={(e) =>
                      patch({ ui_font_scale: Number(e.target.value) / 100 })
                    }
                  >
                    {Array.from(
                      { length: 12 },
                      (_, index) => 85 + index * 5,
                    ).map((value) => (
                      <option key={value} value={value}>
                        {value}%
                      </option>
                    ))}
                  </select>
                </label>
                {check("use_ssh_config", "读取本机 SSH config")}
              </>
            )}
            {current === "terminal" && (
              <>
                {check("show_line_numbers", "显示行号")}
                {check("show_timestamps", "显示时间戳")}
                {check("terminal_wrap", "自动换行")}
                <label>
                  {t("终端字体")}
                  <select
                    value={draft.font_family}
                    disabled={fonts.loading}
                    onChange={(e) => patch({ font_family: e.target.value })}
                  >
                    {fontOptions(
                      draft.font_family,
                      fonts.catalog?.families ?? [],
                    ).map((font) => (
                      <option key={font} value={font}>
                        {font}
                      </option>
                    ))}
                  </select>
                </label>
                <FontStatus fonts={fonts} />
                <SettingInput
                  label={t("终端字号")}
                  type="number"
                  min={8}
                  max={48}
                  step={0.5}
                  value={String(draft.font_size)}
                  commit={(value) => patch({ font_size: Number(value) })}
                />
                <div
                  className="settings-font-preview"
                  aria-label={t("字体预览")}
                  style={{
                    fontFamily: draft.font_family,
                    fontSize: draft.font_size,
                  }}
                >
                  Aa Bb 0123456789
                  <br />
                  中文终端 · $ ssh user@host
                </div>
                <SettingInput
                  label={t("滚动缓冲行数")}
                  type="number"
                  min={0}
                  max={100000}
                  step={1}
                  value={String(draft.scrollback_lines)}
                  commit={(value) => patch({ scrollback_lines: Number(value) })}
                />
                <label>
                  {t("光标")}
                  <select
                    value={draft.cursor_style}
                    onChange={(e) => patch({ cursor_style: e.target.value })}
                  >
                    <option value="block">{t("方块")}</option>
                    <option value="bar">{t("竖线")}</option>
                    <option value="underline">{t("下划线")}</option>
                  </select>
                </label>
                <SettingInput
                  multiline
                  label={t("提示关键字")}
                  hint={t("每行一个，忽略英文大小写")}
                  value={(draft.trigger_highlights ?? []).join("\n")}
                  commit={(value) =>
                    patch({
                      trigger_highlights: value
                        .split("\n")
                        .map((item) => item.trim())
                        .filter(Boolean),
                    })
                  }
                />
              </>
            )}
            {current === "editor" && !phone && (
              <>
                <label>
                  {t("默认编辑器")}
                  <select
                    value={draft.default_editor ?? ""}
                    onChange={(e) => patch({ default_editor: e.target.value })}
                  >
                    <option value="">{t("系统默认程序")}</option>
                    {[...editors, ...(draft.editors ?? [])]
                      .filter(
                        (item, index, all) =>
                          all.findIndex(
                            (other) => other.command === item.command,
                          ) === index,
                      )
                      .map((item) => (
                        <option key={item.command} value={item.command}>
                          {item.command === "__system__"
                            ? t("系统默认程序")
                            : item.name}
                        </option>
                      ))}
                    {draft.default_editor &&
                      ![...editors, ...(draft.editors ?? [])].some(
                        (item) => item.command === draft.default_editor,
                      ) && (
                        <option value={draft.default_editor}>
                          {draft.default_editor}
                        </option>
                      )}
                  </select>
                </label>
                <h3>{t("自定义编辑器")}</h3>
                {(draft.editors ?? []).map((item) => (
                  <div className="settings-editor-row" key={item.command}>
                    <span>
                      <strong>{item.name}</strong>
                      <code>{item.command}</code>
                    </span>
                    <button
                      onClick={() =>
                        patch({
                          editors: (draft.editors ?? []).filter(
                            (other) => other.command !== item.command,
                          ),
                          ...(draft.default_editor === item.command
                            ? { default_editor: "" }
                            : {}),
                        })
                      }
                    >
                      {t("删除")}
                    </button>
                  </div>
                ))}
                <label>
                  {t("名称")}
                  <input
                    value={editorName}
                    maxLength={256}
                    onChange={(e) => setEditorName(e.target.value)}
                  />
                </label>
                <label>
                  {t("命令")}
                  <input
                    value={editorCommand}
                    maxLength={4096}
                    placeholder={'code --wait "{file}"'}
                    onChange={(e) => setEditorCommand(e.target.value)}
                  />
                </label>
                <p className="muted">
                  {t("命令中的 {file} 将替换为本地副本路径，不经过 shell。")}
                </p>
                <button
                  onClick={addEditor}
                  disabled={(draft.editors?.length ?? 0) >= 64}
                >
                  {t("添加")}
                </button>
                {editorError && (
                  <p role="alert" className="error">
                    {editorError}
                  </p>
                )}
              </>
            )}
            {current === "sync" && <SyncPanel />}
            {current === "update" && !info?.mobile && (
              <UpdatePanel draft={updates} canInstall={canInstall} />
            )}
          </div>
        </div>
      )}
      {(current !== "update" ||
        writer.busy ||
        writer.dirty ||
        !!writer.error) && (
        <footer className="settings-save-state">
          <p role="status" className={writer.error ? "error" : "muted"}>
            {writer.error
              ? conflict
                ? t("此设置已被其他操作修改，请重新加载，或确认使用当前值。")
                : errorText(writer.error)
              : writer.busy
                ? t("保存中…")
                : writer.dirty
                  ? t("有尚未保存的设置")
                  : t("设置已保存")}
          </p>
          {!!writer.error && (
            <button
              disabled={writer.busy}
              onClick={() => void writer.retry(conflict)}
            >
              {t(conflict ? "使用当前值" : "重试保存")}
            </button>
          )}
          <button
            disabled={writer.busy}
            onClick={() => {
              if (
                !writer.dirty ||
                window.confirm(t("重新加载会丢弃尚未保存的设置，是否继续？"))
              )
                void writer.reload();
            }}
          >
            {t("重新加载设置")}
          </button>
        </footer>
      )}
    </section>
  );
}

function SettingInput({
  label,
  value,
  commit,
  multiline,
  hint,
  ...props
}: {
  label: string;
  value: string;
  commit: (value: string) => void;
  multiline?: boolean;
  hint?: string;
} & Omit<
  InputHTMLAttributes<HTMLInputElement>,
  "value" | "onChange" | "onBlur"
>) {
  const t = useText();
  const saved = useRef(value);
  const [text, setText] = useProtectedState(
    value,
    (next) => next !== saved.current,
  );
  const focused = useRef(false);
  const commitAtFocus = useRef(commit);
  useEffect(() => {
    if (!focused.current) {
      saved.current = value;
      setText(value);
    }
  }, [value, setText]);
  const save = () => {
    focused.current = false;
    if (text !== saved.current) {
      saved.current = text;
      commitAtFocus.current(text);
      setText(text);
    }
  };
  return (
    <label>
      {label}
      {multiline ? (
        <textarea
          rows={5}
          value={text}
          onFocus={() => {
            focused.current = true;
            commitAtFocus.current = commit;
          }}
          onChange={(e) => setText(e.target.value)}
          onBlur={save}
        />
      ) : (
        <input
          {...props}
          value={text}
          onFocus={() => {
            focused.current = true;
            commitAtFocus.current = commit;
          }}
          onChange={(e) => setText(e.target.value)}
          onBlur={(event) => {
            if (event.currentTarget.checkValidity()) save();
            else event.currentTarget.reportValidity();
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.nativeEvent.isComposing) {
              event.preventDefault();
              event.currentTarget.blur();
            }
          }}
        />
      )}
      {hint && <small>{hint}</small>}
      <span className="sr-only">{t("输入后按 Enter 或移开焦点保存")}</span>
    </label>
  );
}

function FontStatus({ fonts }: { fonts: ReturnType<typeof useFonts> }) {
  const t = useText();
  const reason = fonts.catalog?.reason;
  const text: TextKey | null = fonts.loading
    ? "正在读取字体列表…"
    : fonts.failed
      ? "读取字体列表失败，请重试。"
      : fonts.catalog?.source === "fallback"
        ? reason === "mobile"
          ? "此设备使用预设字体列表。"
          : reason === "unavailable"
            ? "此设备暂不支持系统字体枚举，已显示回退字体。"
            : "无法读取系统字体，已显示回退字体。"
        : null;
  if (!text) return null;
  return (
    <div>
      <p role="status" className={fonts.failed ? "error" : "muted"}>
        {t(text)}
      </p>
      {!fonts.loading &&
        (fonts.failed ||
          (fonts.catalog?.source === "fallback" && reason !== "mobile")) && (
          <button onClick={fonts.retry}>{t("重新读取字体")}</button>
        )}
    </div>
  );
}
