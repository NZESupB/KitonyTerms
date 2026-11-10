import assert from "node:assert/strict";
import test from "node:test";
import {
  operationRequest,
  operationSupported,
  listOperation,
} from "./operations";

test("运维能力按真实列表后端判定，proc 可读不会误开放功能", () => {
  const caps = {
    linux: true,
    systemd: false,
    ps: false,
    proc: true,
    ss: false,
    proc_net: true,
    docker_daemon: true,
    compose: false,
  };
  assert.equal(operationSupported(caps, "processes", ""), false);
  assert.equal(operationSupported(caps, "network", ""), false);
  assert.equal(operationSupported(caps, "docker", "containers"), true);
  assert.equal(operationSupported(caps, "docker", "compose"), false);
  assert.equal(
    operationSupported({ ...caps, linux: false }, "docker", "containers"),
    false,
  );
  assert.equal(operationSupported(undefined, "services", ""), false);
});
test("运维请求初始化完整且独立分配身份，Docker 列表保留资源归属", () => {
  const first = operationRequest({
    kind: "service_action",
    target: "ssh.service",
    action: "restart",
  });
  const second = operationRequest({ kind: "services" });
  assert.notEqual(first.id, second.id);
  assert.deepEqual(first.targets, []);
  assert.equal(first.pid, 0);
  assert.equal(first.target, "ssh.service");
  assert.deepEqual(listOperation("docker", "images"), {
    kind: "docker_list",
    resource: "images",
  });
  assert.deepEqual(listOperation("processes", "containers"), {
    kind: "processes",
  });
});
