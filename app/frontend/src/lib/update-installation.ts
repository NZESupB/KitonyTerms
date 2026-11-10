export type InstallStage =
  | "idle"
  | "preparing"
  | "cancelling"
  | "committing"
  | "committed"
  | "uncertain";
export type InstallProblem =
  | "blocked"
  | "busy"
  | "failed"
  | "cleanup"
  | "uncertain";
export class InstallError extends Error {
  constructor(readonly kind: InstallProblem) {
    super(kind);
  }
}
export type InstallQuit = {
  requestId: string;
  intent: string;
  committing: boolean;
};
export type InstallPort = {
  read: () => PromiseLike<InstallQuit>;
  prepare: () => PromiseLike<string>;
  commit: (id: string) => PromiseLike<void>;
  abort: (id: string) => PromiseLike<void>;
  changed: (stage: InstallStage) => void;
};
type Attempt = {
  id: string;
  started: boolean;
  settled: boolean;
  cancelled: boolean;
  submitted: boolean;
  cleanup?: Promise<void>;
  done?: Promise<boolean>;
};

// 实例只持有协议所有权。准备调用不做传输层取消，以便保留迟到的精确请求 ID。
export class UpdateInstaller {
  stage: InstallStage = "idle";
  private attempt?: Attempt;
  constructor(private port: InstallPort) {}
  private change(stage: InstallStage) {
    this.stage = stage;
    this.port.changed(stage);
  }
  get active() {
    return this.attempt !== undefined;
  }
  get submitted() {
    return this.attempt?.submitted ?? false;
  }
  private release(a: Attempt) {
    if (this.attempt !== a) return;
    this.attempt = undefined;
    this.change("idle");
  }
  private cleanup(a: Attempt): Promise<void> {
    if (!a.started) return Promise.resolve();
    if (a.cleanup) return a.cleanup;
    a.cleanup = (async () => {
      const state = await this.port.read();
      if (
        a.submitted &&
        state.requestId === a.id &&
        state.intent === "install"
      ) {
        throw new InstallError("uncertain");
      }
      if (!a.id) {
        // 调用失败没有返回 ID 时，不能推断当前请求属于本次准备。
        if (state.intent === "install" && state.requestId)
          throw new InstallError("uncertain");
        return;
      }
      if (state.requestId !== a.id || state.intent !== "install") return;
      if (state.committing) throw new InstallError("uncertain");
      let failed = false;
      try {
        await this.port.abort(a.id);
      } catch {
        failed = true;
      }
      const current = await this.port.read();
      if (current.requestId === a.id && current.intent === "install") {
        throw new InstallError(current.committing ? "uncertain" : "cleanup");
      }
      // 服务端即使保留失败材料，也会先恢复业务准入；仍须向页面报告回收错误。
      if (failed) throw new InstallError("failed");
    })().finally(() => {
      a.cleanup = undefined;
    });
    return a.cleanup;
  }
  start(safe: () => boolean): Promise<boolean> {
    if (this.attempt) return Promise.resolve(false);
    if (!safe()) return Promise.reject(new InstallError("blocked"));
    const a: Attempt = {
      id: "",
      started: false,
      settled: false,
      cancelled: false,
      submitted: false,
    };
    this.attempt = a;
    this.change("preparing");
    a.done = this.run(a, safe);
    return a.done;
  }
  private async run(a: Attempt, safe: () => boolean): Promise<boolean> {
    try {
      const previous = await this.port.read();
      if (previous.requestId) throw new InstallError("busy");
      if (a.cancelled) return false;
      if (!safe()) throw new InstallError("blocked");
      a.started = true;
      a.id = await this.port.prepare();
      if (!a.id) throw new InstallError("failed");
      if (a.cancelled || !safe()) {
        this.change("cancelling");
        throw new InstallError("blocked");
      }
      a.submitted = true;
      this.change("committing");
      await this.port.commit(a.id);
      this.change("committed");
      return true;
    } catch (error) {
      try {
        await this.cleanup(a);
      } catch (cleanupError) {
        // 明确清理完成但留有材料时，服务端已恢复准入，页面也可继续使用。
        if (
          cleanupError instanceof InstallError &&
          cleanupError.kind === "failed"
        ) {
          this.release(a);
          throw cleanupError;
        }
        this.change("uncertain");
        throw cleanupError instanceof InstallError
          ? cleanupError
          : new InstallError("uncertain");
      }
      if (a.cancelled) return false;
      throw error instanceof InstallError ? error : new InstallError("failed");
    } finally {
      a.settled = true;
      if (
        this.attempt === a &&
        this.stage !== "committed" &&
        this.stage !== "uncertain"
      )
        this.release(a);
    }
  }
  async cancel(): Promise<void> {
    const a = this.attempt;
    if (!a || a.submitted) return;
    a.cancelled = true;
    this.change("cancelling");
    // 未返回 ID 的准备仍在运行，必须等它结束后由 run 精确回收；不提前解冻。
    if (!a.settled && a.done) {
      await a.done;
      return;
    }
    try {
      await this.cleanup(a);
      this.release(a);
    } catch (error) {
      if (error instanceof InstallError && error.kind === "failed")
        this.release(a);
      else this.change("uncertain");
      throw error;
    }
  }
  async reconcile(): Promise<void> {
    const a = this.attempt;
    if (
      !a ||
      !a.settled ||
      (this.stage !== "uncertain" && !(this.stage === "committing" && !a.done))
    )
      return;
    const state = await this.port.read();
    if (
      !state.requestId ||
      state.intent !== "install" ||
      (a.id && state.requestId !== a.id)
    )
      this.release(a);
  }
  // 新页面没有本地调用时，显式接管它读取到的遗留意图；提交中的请求只能等待退出。
  async recover(state: InstallQuit): Promise<void> {
    if (this.attempt || !state.requestId || state.intent !== "install") return;
    const a: Attempt = {
      id: state.requestId,
      started: true,
      settled: true,
      cancelled: true,
      submitted: state.committing,
    };
    this.attempt = a;
    if (a.submitted) {
      this.change("committing");
      return;
    }
    this.change("cancelling");
    await this.cancel();
  }
}
