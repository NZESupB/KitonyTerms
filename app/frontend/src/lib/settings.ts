import type { ConfigSnapshot, Settings } from "./backend";

type Field = keyof Settings;
type Change = { before: unknown; value: unknown };
export class SettingsConflict extends Error {}
export class SettingsWriter {
  private pending = new Map<Field, Change>();
  private running = false;
  private failure: unknown;
  constructor(
    private readonly transport: {
      load: () => Promise<ConfigSnapshot>;
      save: (
        snapshot: ConfigSnapshot,
        settings: Settings,
      ) => Promise<ConfigSnapshot>;
      apply: (snapshot: ConfigSnapshot) => void;
      changed: () => void;
    },
  ) {}
  get busy() {
    return this.running;
  }
  get error() {
    return this.failure;
  }
  get dirty() {
    return this.pending.size > 0;
  }
  view(settings: Settings): Settings {
    return {
      ...settings,
      ...Object.fromEntries(
        [...this.pending].map(([field, change]) => [field, change.value]),
      ),
    };
  }
  update(settings: Settings, patch: Partial<Settings>): void {
    for (const field of Object.keys(patch) as Field[]) {
      const previous = this.pending.get(field);
      this.pending.set(field, {
        before: previous ? previous.before : settings[field],
        value: patch[field],
      });
    }
    this.transport.changed();
    if (!this.failure) void this.flush();
  }
  async reload(): Promise<void> {
    if (this.running) return;
    try {
      const snapshot = await this.transport.load();
      this.transport.apply(snapshot);
      this.pending.clear();
      this.failure = undefined;
    } catch (error) {
      this.failure = error;
    }
    this.transport.changed();
  }
  async retry(overwrite = false): Promise<void> {
    if (this.running) return;
    this.failure = undefined;
    await this.flush(overwrite);
  }
  private async flush(overwrite = false): Promise<void> {
    if (this.running || !this.pending.size) return;
    this.running = true;
    this.transport.changed();
    try {
      while (this.pending.size) {
        const changes = new Map(this.pending);
        let result: ConfigSnapshot | undefined;
        // 只对版本竞争重试一次；同字段外部修改必须明确处理。
        for (let attempt = 0; attempt < 2; attempt++) {
          const snapshot = await this.transport.load();
          this.transport.apply(snapshot);
          const settings = { ...snapshot.config.settings };
          for (const [field, change] of changes) {
            if (
              !overwrite &&
              !equal(settings[field], change.before) &&
              !equal(settings[field], change.value)
            )
              throw new SettingsConflict(
                "此设置已被其他操作修改，请重新加载，或确认使用当前值。",
              );
            Object.assign(settings, { [field]: change.value });
          }
          try {
            result = await this.transport.save(snapshot, settings);
            break;
          } catch (error) {
            const current = await this.transport.load();
            this.transport.apply(current);
            if (attempt > 0 || current.revision === snapshot.revision)
              throw error;
          }
        }
        if (!result) throw new Error("设置保存未返回结果");
        this.transport.apply(result);
        for (const [field, sent] of changes) {
          const current = this.pending.get(field);
          if (current === sent) this.pending.delete(field);
          else if (current)
            this.pending.set(field, {
              before: result.config.settings[field],
              value: current.value,
            });
        }
        overwrite = false;
      }
    } catch (error) {
      this.failure = error;
    } finally {
      this.running = false;
      this.transport.changed();
    }
  }
}
function equal(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}
