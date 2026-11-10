import test from "node:test";
import assert from "node:assert/strict";
import {
  clampPercent,
  latencyLevel,
  metricPercent,
  metricRate,
  metricUptime,
  networkActivity,
} from "./monitor";

test("监控未知、已知零与大整数分别显示", () => {
  assert.equal(metricPercent("", "10"), null);
  assert.equal(metricPercent("0", "0"), null);
  assert.equal(metricPercent("0", "100"), 0);
  assert.equal(
    metricPercent("18446744073709551614", "18446744073709551615"),
    99.9,
  );
  assert.equal(metricPercent("150", "100"), 100);
  assert.equal(metricRate(""), "--");
  assert.equal(metricRate("0"), "0 B/s");
  assert.equal(metricRate("2048"), "2.0 KiB/s");
  assert.equal(metricUptime("9007199254740993"), "104249991374d 7h");
  assert.equal(metricUptime("90000"), "1d 1h");
  assert.equal(metricUptime("0"), "0s");
  assert.equal(metricUptime(""), "--");
});
test("监控网络刻度与延迟阈值沿用原规则", () => {
  assert.equal(networkActivity("0", "0"), 0);
  assert.equal(networkActivity("", "0"), null);
  assert.equal(networkActivity("65536", "0"), 25);
  assert.equal(networkActivity("524288", "524288"), 50);
  assert.equal(networkActivity("10485760", "0"), 75);
  assert.equal(
    networkActivity("18446744073709551615", "18446744073709551615"),
    100,
  );
  assert.equal(latencyLevel(""), "unknown");
  assert.equal(latencyLevel("0"), "unknown");
  assert.equal(latencyLevel("149"), "normal");
  assert.equal(latencyLevel("150"), "warn");
  assert.equal(latencyLevel("299"), "warn");
  assert.equal(latencyLevel("300"), "danger");
  assert.equal(clampPercent(Number.NaN), 0);
  assert.equal(clampPercent(150), 100);
});
