import { Backend, useApp, hasLocalInstallProtection } from "./backend";
import {
  UpdateInstaller,
  InstallError,
  type InstallStage,
} from "./update-installation";
import type { TextKey } from "./i18n";
import { translate } from "./i18n";

export const installProblems: Record<string, TextKey> = {
  blocked: "请先结束文件编辑、文件操作、未保存设置或配置同步。",
  busy: "已有退出或安装请求，请先完成当前操作。",
  failed: "更新安装准备或提交失败，应用继续运行。",
  cleanup: "更新准备尚未回收，请重试取消。",
  uncertain: "正在确认安装状态，请勿重复提交。",
};
export function reportInstall(error: unknown) {
  const language =
    useApp.getState().config?.config.settings.language ?? "chinese";
  const key =
    installProblems[error instanceof InstallError ? error.kind : "failed"];
  useApp.setState({ error: translate(language, key) });
}
export const updateInstaller = new UpdateInstaller({
  read: () => Backend.GetQuitState(),
  prepare: () => Backend.PrepareUpdateInstall(),
  commit: (id) => Backend.CommitUpdateInstall(id),
  abort: (id) => Backend.AbortUpdateInstall(id),
  changed: (stage: InstallStage) =>
    useApp.setState((state) => {
      const language = state.config?.config.settings.language ?? "chinese";
      const ownError = Object.values(installProblems).some(
        (key) => translate(language, key) === state.error,
      );
      return {
        installing: stage !== "idle",
        installStage: stage,
        backendEpoch:
          state.backendEpoch + (stage === "idle" && state.installing ? 1 : 0),
        error:
          ownError && (stage === "idle" || stage === "preparing")
            ? ""
            : state.error,
      };
    }),
});
export function startUpdateInstall(
  canInstall: () => boolean,
): Promise<boolean> {
  return updateInstaller.start(() => {
    const state = useApp.getState();
    return (
      !!state.info &&
      !state.info.mobile &&
      !state.quitting &&
      document.visibilityState === "visible" &&
      canInstall() &&
      !hasLocalInstallProtection()
    );
  });
}
