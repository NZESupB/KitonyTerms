import type { SecretReveal } from "./backend";

type Request = PromiseLike<SecretReveal> & { cancel(): Promise<unknown> };
export type SecretViewState = {
  owner: string;
  value: string | null;
  busy: boolean;
  failure: string;
};

// 仅由凭据字段实例持有；明文不进入应用store、通知或持久化。
export class SecretView {
  state: SecretViewState = { owner: "", value: null, busy: false, failure: "" };
  private epoch = 0;
  private pending?: Request;
  private timer?: ReturnType<typeof setTimeout>;
  constructor(
    private changed: (state: SecretViewState) => void,
    private schedule = (callback: () => void) => setTimeout(callback, 30_000),
  ) {}
  private publish(value: Partial<SecretViewState>) {
    this.state = { ...this.state, ...value };
    this.changed(this.state);
  }
  clear() {
    this.epoch++;
    clearTimeout(this.timer);
    this.timer = undefined;
    const previous = this.pending;
    this.pending = undefined;
    void previous?.cancel().catch(() => {});
    this.publish({ value: null, busy: false, failure: "" });
  }
  own(owner: string) {
    if (this.state.owner === owner) return;
    this.clear();
    this.publish({ owner });
  }
  async reveal(factory: () => Request) {
    if (this.state.busy || !this.state.owner) return;
    this.clear();
    const epoch = this.epoch;
    this.publish({ busy: true });
    try {
      const request = factory();
      this.pending = request;
      const result = await request;
      if (epoch !== this.epoch || this.pending !== request) return;
      this.pending = undefined;
      if (result.failure)
        this.publish({ busy: false, failure: result.failure });
      else if (
        !result.value ||
        new TextEncoder().encode(result.value).length > 65536
      )
        this.publish({ busy: false, failure: "failed" });
      else {
        this.publish({ value: result.value, busy: false });
        this.timer = this.schedule(() => this.clear());
      }
    } catch {
      if (epoch === this.epoch) {
        this.pending = undefined;
        this.publish({ busy: false, failure: "failed" });
      }
    }
  }
}
