import { useEffect } from "react";
import * as Dialog from "@radix-ui/react-dialog";
import { useApp } from "../lib/backend";
import { useText } from "../lib/useText";
import { reportInstall, updateInstaller } from "../lib/update-install";
import type { InstallStage } from "../lib/update-installation";
import type { TextKey } from "../lib/i18n";

const messages: Record<InstallStage, TextKey> = {
  idle: "更新状态",
  preparing: "正在准备更新安装…",
  cancelling: "正在取消安装准备…",
  committing: "正在回收连接并提交安装…",
  committed: "安装已提交，正在退出…",
  uncertain: "正在确认安装状态，请勿重复提交。",
};
export function UpdateInstallStatus() {
  const t = useText();
  const stage = useApp((state) => state.installStage);
  const cancel = () => {
    void updateInstaller.cancel().catch(reportInstall);
  };
  useEffect(() => {
    const hidden = () => {
      if (document.visibilityState !== "visible")
        void updateInstaller.cancel().catch(reportInstall);
    };
    const leaving = () => {
      void updateInstaller.cancel().catch(reportInstall);
    };
    document.addEventListener("visibilitychange", hidden);
    window.addEventListener("pagehide", leaving);
    return () => {
      document.removeEventListener("visibilitychange", hidden);
      window.removeEventListener("pagehide", leaving);
      leaving();
    };
  }, []);
  const cancellable =
    stage === "preparing" ||
    (stage === "uncertain" && !updateInstaller.submitted);
  return (
    <Dialog.Root open={stage !== "idle"}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay install-overlay" />
        <Dialog.Content
          className="dialog-content install-status"
          data-install-status
          onEscapeKeyDown={(event) => {
            event.preventDefault();
            if (cancellable) cancel();
          }}
          onPointerDownOutside={(event) => event.preventDefault()}
          onInteractOutside={(event) => event.preventDefault()}
        >
          <header>
            <Dialog.Title>{t("更新状态")}</Dialog.Title>
          </header>
          <Dialog.Description>
            {t("安装会断开当前 SSH 连接并重启应用。")}
          </Dialog.Description>
          <p role="status" aria-live="polite">
            {t(messages[stage])}
          </p>
          <footer>
            {cancellable && (
              <button onClick={cancel}>{t("取消安装准备")}</button>
            )}
            {stage === "uncertain" && (
              <button onClick={() => window.location.reload()}>
                {t("重新加载")}
              </button>
            )}
          </footer>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
