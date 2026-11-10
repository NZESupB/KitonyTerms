import assert from "node:assert/strict";
import test from "node:test";
import type { ConfigSnapshot, Settings } from "./backend";
import { SettingsConflict, SettingsWriter } from "./settings";
const initial = () =>
  ({
    revision: "1",
    config: {
      settings: {
        language: "chinese",
        theme: "default-dark",
        font_size: 13,
        sftp_auto_sync: false,
        sftp_column_widths: [],
      },
      sessions: [],
      groups: [],
    },
  }) as unknown as ConfigSnapshot;
async function settled(writer: SettingsWriter) {
  for (let count = 0; count < 100 && writer.busy; count++)
    await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(writer.busy, false);
}
function fixture() {
  let snapshot = initial();
  let active = 0,
    maxActive = 0,
    calls = 0;
  let beforeSave: (() => Promise<void>) | undefined;
  const writer = new SettingsWriter({
    load: async () => structuredClone(snapshot),
    save: async (base, settings) => {
      active++;
      maxActive = Math.max(maxActive, active);
      calls++;
      try {
        await beforeSave?.();
        if (base.revision !== snapshot.revision)
          throw new Error("version conflict");
        snapshot = {
          ...snapshot,
          revision: String(BigInt(snapshot.revision) + 1n),
          config: { ...snapshot.config, settings },
        };
        return structuredClone(snapshot);
      } finally {
        active--;
      }
    },
    apply: () => {},
    changed: () => {},
  });
  return {
    writer,
    get: () => snapshot,
    calls: () => calls,
    max: () => maxActive,
    intercept: (fn?: () => Promise<void>) => {
      beforeSave = fn;
    },
    external: (patch: Partial<Settings>) => {
      snapshot = {
        ...snapshot,
        revision: String(BigInt(snapshot.revision) + 1n),
        config: {
          ...snapshot.config,
          settings: { ...snapshot.config.settings, ...patch },
        },
      };
    },
  };
}
test("设置连续修改串行，保存期间的新值保留，合并其他字段", async () => {
  const f = fixture();
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  f.intercept(() => gate);
  const settings = f.get().config.settings;
  f.writer.update(settings, { font_size: 14 });
  await new Promise<void>((resolve) => setImmediate(resolve));
  f.writer.update(settings, { font_size: 15, language: "english" });
  f.external({ sftp_auto_sync: true, sftp_column_widths: [120, 140] });
  release();
  await settled(f.writer);
  assert.equal(f.writer.error, undefined);
  assert.equal(f.writer.dirty, false);
  assert.equal(f.max(), 1);
  assert.equal(f.get().config.settings.font_size, 15);
  assert.equal(f.get().config.settings.language, "english");
  assert.equal(f.get().config.settings.sftp_auto_sync, true);
  assert.deepEqual(f.get().config.settings.sftp_column_widths, [120, 140]);
});
test("同字段外部修改保留草稿并等待显式覆盖", async () => {
  const f = fixture(),
    settings = f.get().config.settings;
  f.external({ theme: "default-light" });
  f.writer.update(settings, { theme: "system" });
  await settled(f.writer);
  assert.ok(f.writer.error instanceof SettingsConflict);
  assert.equal(f.calls(), 0);
  assert.equal(f.writer.view(settings).theme, "system");
  await f.writer.retry();
  assert.ok(f.writer.error instanceof SettingsConflict);
  await f.writer.retry(true);
  assert.equal(f.get().config.settings.theme, "system");
  assert.equal(f.writer.dirty, false);
});
test("保存失败不循环，重试与重新加载分别保留和丢弃草稿", async () => {
  const f = fixture(),
    settings = f.get().config.settings;
  f.intercept(async () => {
    throw new Error("disk full");
  });
  f.writer.update(settings, { font_size: 16 });
  await settled(f.writer);
  assert.equal(f.calls(), 1);
  assert.match(String(f.writer.error), /disk full/);
  f.writer.update(settings, { language: "english" });
  await settled(f.writer);
  assert.equal(f.calls(), 1);
  f.intercept();
  await f.writer.retry();
  assert.equal(f.get().config.settings.font_size, 16);
  assert.equal(f.get().config.settings.language, "english");
  f.intercept(async () => {
    throw new Error("disk full");
  });
  f.writer.update(f.get().config.settings, { font_size: 17 });
  await settled(f.writer);
  await f.writer.reload();
  assert.equal(f.writer.dirty, false);
  assert.equal(f.writer.error, undefined);
  assert.equal(f.writer.view(f.get().config.settings).font_size, 16);
});
