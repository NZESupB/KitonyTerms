import assert from "node:assert/strict";
import test from "node:test";
import type { UpdateSnapshot } from "./backend";
import {
  UpdateDraft,
  updateDate,
  updatePercent,
  validUpdatePreferences,
} from "./update";

function snapshot(sequence = "1", preferenceRevision = "10"): UpdateSnapshot {
  return {
    sequence,
    preferenceRevision,
    phase: "available",
    current: {
      channel: "stable",
      version: "0.1.17",
      commit: "a",
      build: "1",
      attempt: "1",
      target: "fixture",
    },
    preferences: {
      auto_check: false,
      auto_download: false,
      proxy: { kind: "direct" },
    },
    available: null,
    received: "0",
    speed: "0",
    lastCheck: "",
    nextCheck: "",
    failure: null,
    notice: "",
    disabledReason: "",
    busy: false,
    installAvailable: false,
    requiresElevation: false,
  };
}

test("更新样本去旧，轮询与冲突保留本机偏好草稿", () => {
  const draft = new UpdateDraft();
  assert.equal(draft.apply(snapshot()), true);
  draft.edit({ auto_check: true });
  assert.equal(draft.dirty, true);
  assert.equal(draft.apply(snapshot("0")), false);
  draft.apply(snapshot("2", "11"));
  assert.equal(draft.preferences?.auto_check, true);
  assert.equal(draft.error, "conflict");
  const request = draft.beginSave();
  assert.equal(request?.expectedRevision, "10");
  draft.failed(new Error("更新偏好版本冲突，请重新加载"));
  assert.equal(draft.dirty, true);
  assert.equal(draft.error, "conflict");
  draft.rebase();
  const retry = draft.beginSave();
  assert.equal(retry?.expectedRevision, "11");
  assert.equal(retry?.preferences.auto_check, true);
  const saved = snapshot("3", "12");
  saved.preferences.auto_check = true;
  draft.saved(saved);
  assert.equal(draft.saving, false);
  assert.equal(draft.dirty, false);
  assert.equal(draft.error, null);
});

test("更新偏好保存失败保留草稿，旧保存响应不能倒退新快照", () => {
  const draft = new UpdateDraft();
  draft.apply(snapshot());
  draft.edit({
    proxy: { kind: "http", host: "127.0.0.1", port: 8080, username: "fixture" },
  });
  const request = draft.beginSave();
  assert.ok(request);
  assert.equal(draft.beginSave(), null);
  draft.edit({ auto_check: true });
  assert.equal(draft.preferences?.auto_check, false);
  draft.failed(new Error("fixture write failed"));
  assert.equal(draft.dirty, true);
  assert.equal(draft.preferences?.proxy.host, "127.0.0.1");
  draft.beginSave();
  const newer = snapshot("5", "12");
  newer.preferences.auto_download = true;
  draft.apply(newer);
  draft.saved(snapshot("4", "11"));
  assert.equal(draft.snapshot?.sequence, "5");
  assert.equal(draft.preferences?.auto_download, true);
  assert.equal(draft.dirty, false);
});

test("更新进度只在摘要通过后100%，未知时间和非法代理显式处理", () => {
  const value = snapshot();
  assert.equal(updatePercent(value), undefined);
  value.available = {
    info: value.current,
    package: { name: "fixture", size: "100", sha256: "a", kind: "dmg" },
    assetId: "1",
    releaseId: "2",
    tag: "fixture",
    notes: "",
    notesTruncated: false,
    publishedAt: "",
  };
  value.received = "100";
  value.phase = "downloading";
  assert.equal(updatePercent(value), 99);
  value.phase = "downloaded";
  assert.equal(updatePercent(value), 100);
  assert.equal(updateDate("invalid", "english"), "--");
  assert.equal(updateDate("99999999999999999999999", "english"), "--");
  assert.equal(
    validUpdatePreferences({
      ...value.preferences,
      proxy: { kind: "http", host: "bad/path", port: 80 },
    }),
    false,
  );
  assert.equal(
    validUpdatePreferences({
      ...value.preferences,
      proxy: { kind: "socks5", host: "::1", port: 1080, username: "fixture" },
    }),
    true,
  );
});

test("更新代理拒绝控制字符，直连清除显式代理字段", () => {
  const preferences = snapshot().preferences;
  for (const text of ["bad\nvalue", "bad\u0085value", "bad\u007fvalue"]) {
    assert.equal(
      validUpdatePreferences({
        ...preferences,
        proxy: { kind: "http", host: text, port: 80 },
      }),
      false,
    );
    assert.equal(
      validUpdatePreferences({
        ...preferences,
        proxy: { kind: "http", host: "127.0.0.1", port: 80, username: text },
      }),
      false,
    );
  }
  const draft = new UpdateDraft();
  draft.apply(snapshot());
  draft.edit({
    proxy: { kind: "direct", host: "unused", port: 80, username: "unused" },
  });
  assert.deepEqual(draft.preferences?.proxy, { kind: "direct" });
  assert.equal(draft.dirty, false);
});

test("无效更新快照显式失败且不清空待保存草稿", () => {
  const draft = new UpdateDraft();
  draft.apply(snapshot());
  draft.edit({ auto_check: true });
  draft.beginSave();
  assert.throws(() => draft.saved(snapshot("invalid")), /更新快照版本无效/);
  draft.failed(new Error("fixture invalid response"));
  assert.equal(draft.dirty, true);
  assert.equal(draft.preferences?.auto_check, true);
});
