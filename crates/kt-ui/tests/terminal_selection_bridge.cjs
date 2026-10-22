'use strict';

// 最小 DOM 仅用于桥生命周期；文本语义由核心测试与真实网格浏览器夹具验证。
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { test } = require('node:test');
const source = fs.readFileSync(path.join(__dirname, '../src/assets/terminal_selection.js'), 'utf8');
const drain = () => new Promise(resolve => setImmediate(resolve));

class Target {
    listeners = [];
    addEventListener(name, handler, { signal } = {}) {
        this.listeners.push({ name, handler, signal });
    }
    dispatch(name, properties = {}) {
        const event = {
            target: this, button: 0, pointerType: 'mouse', pointerId: 1,
            clientX: 35, clientY: 50, timeStamp: 1000, defaultPrevented: false,
            preventDefault() { this.defaultPrevented = true; },
            stopImmediatePropagation() {}, ...properties,
        };
        for (const listener of [...this.listeners]) {
            if (listener.name === name && !listener.signal?.aborted) listener.handler(event);
        }
        return event;
    }
    get activeListeners() { return this.listeners.filter(item => !item.signal?.aborted).length; }
}

function fixture(t, missing = null) {
    const window = new Target();
    const document = new Target();
    const surface = new Target();
    const rect = { left: 0, top: 0, right: 800, bottom: 200, width: 800, height: 200 };
    const cell = { dataset: { columns: '1' }, getBoundingClientRect: () => ({ width: 10 }) };
    const row = { querySelector: () => cell, getBoundingClientRect: () => ({ height: 20 }) };
    const screen = {
        dataset: { rows: '10', cols: '80', revision: '1', displayOffset: '0' },
        contains: node => node === screen,
        querySelector: () => row, getBoundingClientRect: () => rect,
    };
    const classes = new Set();
    const captured = new Set();
    Object.assign(surface, {
        isConnected: true, scrollLeft: 0,
        classList: { add: name => classes.add(name), remove: name => classes.delete(name) },
        focus: () => { document.activeElement = surface; },
        contains: node => node === screen || node === surface,
        getBoundingClientRect: () => rect,
        setPointerCapture: id => captured.add(id),
        hasPointerCapture: id => captured.has(id),
        releasePointerCapture(id) {
            captured.delete(id);
            surface.dispatch('lostpointercapture', { pointerId: id });
        },
    });
    document.body = {};
    document.activeElement = document.body;
    document.getElementById = id => {
        const kind = id.endsWith('-screen') ? 'screen' : 'surface';
        return missing === kind ? null : kind === 'screen' ? screen : surface;
    };
    window.getSelection = () => null;
    const frames = new Map();
    const observers = new Set();
    const channels = new Map();
    const errors = [];
    const clipboard = [];
    let frameId = 0;
    let queryId = 0;
    const context = vm.createContext({
        window, document, AbortController, performance,
        navigator: { platform: 'MacIntel', clipboard: { writeText: async text => clipboard.push(text) } },
        console: { error: (...args) => errors.push(args.join(' ')) },
        MutationObserver: class {
            observe() { observers.add(this); }
            disconnect() { observers.delete(this); }
        },
        requestAnimationFrame: callback => { frames.set(++frameId, callback); return frameId; },
        cancelAnimationFrame: id => frames.delete(id),
    });
    const AsyncFunction = vm.runInContext('(async function() {}).constructor', context);
    const bridges = [];
    function install(bridgeKey = 1) {
        const id = ++queryId;
        const waiting = [];
        const queued = [];
        const bridge = { id, actions: [], ends: 0, finished: false };
        const channel = {
            send(action) {
                if (action === null) bridge.ends++;
                else bridge.actions.push(action);
            },
            recv: () => queued.length ? Promise.resolve(queued.shift()) : new Promise(resolve => waiting.push(resolve)),
            rustSend(reply) {
                if (waiting.length) waiting.shift()(reply);
                else queued.push(reply);
            },
        };
        channels.set(id, channel);
        // 对应 Dioxus Desktop 0.7.10 query.rs：AsyncFunction 返回后 close，
        // Rust 回复必须通过仍存活的 query 注册项查找，不能直接持有模拟通道绕过它。
        bridge.done = new AsyncFunction('dioxus', 'terminalId', 'bridgeKey', source)(channel, 'terminal-test', bridgeKey)
            .then(() => { channels.delete(id); bridge.finished = true; });
        bridge.controller = surface.__ktSelection;
        bridge.reply = (text = null) => {
            assert.ok(channels.has(id), 'eval 提前结束，Rust 回复通道已关闭');
            channel.rustSend({ Ok: { accepted: true, revision: 1, text } });
        };
        // 已在 JS 消息队列中的回复可能与 cleanup 竞争。
        bridge.deliverQueuedReply = text => channel.rustSend({ Ok: { accepted: true, revision: 1, text } });
        bridges.push(bridge);
        return bridge;
    }
    t.after(async () => {
        for (const bridge of bridges) bridge.controller?.cleanup();
        await drain();
    });
    return {
        install, surface, screen, document, window, classes, captured, channels, frames, observers, errors, clipboard,
        pointer: (name, properties = {}) => surface.dispatch(name, { target: screen, ...properties }),
        frame() {
            const current = [...frames.values()];
            frames.clear();
            for (const callback of current) callback(10);
        },
    };
}

function assertLive(f, bridge) {
    assert.equal(bridge.finished, false, '监听安装后 eval 必须保持 pending');
    assert.equal(f.channels.has(bridge.id), true);
}

async function dragAndCopy(f, bridge, x = 85) {
    const first = bridge.actions.length;
    assert.equal(f.pointer('pointerdown').defaultPrevented, true);
    await drain();
    assert.equal(bridge.actions[first].type, 'begin');
    bridge.reply();
    await drain();
    f.pointer('pointermove', { clientX: x });
    f.frame();
    await drain();
    assert.equal(bridge.actions[first + 1].type, 'extend');
    // 保留一个在途 extend，松开与立即复制必须排在它之后。
    f.pointer('pointerup', { clientX: x + 10 });
    const copying = bridge.controller.copy();
    assert.equal(bridge.actions.length, first + 2);
    bridge.reply();
    await drain();
    assert.equal(bridge.actions[first + 2].type, 'extend');
    assert.equal(bridge.actions[first + 2].point.column, Math.floor((x + 10) / 10));
    bridge.reply();
    await drain();
    assert.equal(bridge.actions[first + 3].type, 'copy');
    bridge.reply('模型文本');
    await copying;
    assert.equal(f.captured.size, 0);
    assertLive(f, bridge);
}

test('eval 持续到 cleanup，连续拖拽与立即复制保持有序', { timeout: 2000 }, async t => {
    const f = fixture(t);
    const bridge = f.install();
    await drain();
    assertLive(f, bridge);
    await dragAndCopy(f, bridge);
    await dragAndCopy(f, bridge, 125);
    assert.deepEqual(f.clipboard, ['模型文本', '模型文本']);
    assert.deepEqual(f.errors, []);
});

test('cleanup 结束 eval 并清理监听、指针捕获和注册表，可重复调用', async t => {
    const f = fixture(t);
    const bridge = f.install();
    await drain();
    assertLive(f, bridge);
    f.pointer('pointerdown');
    bridge.controller.cleanup();
    await bridge.done;
    bridge.controller.cleanup();
    assert.equal(bridge.ends, 1);
    assert.equal(f.surface.__ktSelection, undefined);
    assert.equal(f.window.__ktTerminalSelectionBridges.size, 0);
    assert.equal(f.channels.size, 0);
    assert.equal(f.classes.size, 0);
    assert.equal(f.captured.size, 0);
    assert.equal(f.frames.size, 0);
    assert.equal(f.observers.size, 0);
    assert.equal(f.surface.activeListeners + f.document.activeListeners + f.window.activeListeners, 0);
    const before = bridge.actions.length;
    assert.equal(f.pointer('pointerdown').defaultPrevented, false);
    await drain();
    assert.equal(bridge.actions.length, before);
});

for (const missing of ['surface', 'screen']) {
    test(`缺少 ${missing} 时正常结束，不留下桥或监听`, async t => {
        const f = fixture(t, missing);
        const bridge = f.install();
        await bridge.done;
        assert.equal(bridge.ends, 1);
        assert.equal(f.channels.size, 0);
        assert.equal(f.window.__ktTerminalSelectionBridges.size, 0);
        assert.equal(f.surface.activeListeners, 0);
    });
}

for (const key of [1, 2]) {
    test(`同一节点重新安装（bridgeKey=${key}）关闭旧桥且新桥可复制`, { timeout: 2000 }, async t => {
        const f = fixture(t);
        const old = f.install();
        await drain();
        assertLive(f, old);
        const current = f.install(key);
        await old.done;
        await drain();
        assertLive(f, current);
        old.controller.cleanup();
        assert.equal(f.surface.__ktSelection, current.controller);
        assert.equal(f.window.__ktTerminalSelectionBridges.size, 1);
        assert.equal(f.observers.size, 1);
        await dragAndCopy(f, current);
        assert.equal(old.actions.length, 0);
        assert.equal(old.ends, 1);
    });
}

test('在途 copy 遇到卸载，迟到回复不写剪贴板且不影响新桥', { timeout: 2000 }, async t => {
    const f = fixture(t);
    const old = f.install();
    await drain();
    assertLive(f, old);
    const rejected = assert.rejects(old.controller.copy(), /终端实例已卸载/);
    await drain();
    assert.equal(old.actions[0].type, 'copy');
    const current = f.install();
    await old.done;
    old.deliverQueuedReply('不应复制');
    await rejected;
    assert.deepEqual(f.clipboard, []);
    await dragAndCopy(f, current);
    assert.deepEqual(f.clipboard, ['模型文本']);
    assert.deepEqual(f.errors, []);
});

test('触摸及终端外事件不被鼠标桥接管', async t => {
    const f = fixture(t);
    const bridge = f.install();
    await drain();
    assertLive(f, bridge);
    assert.equal(f.pointer('pointerdown', { pointerType: 'touch' }).defaultPrevented, false);
    assert.equal(f.pointer('pointerdown', { target: {} }).defaultPrevented, false);
    assert.equal(f.document.dispatch('copy').defaultPrevented, false);
    await drain();
    assert.equal(bridge.actions.length, 0);
    assert.equal(f.classes.size, 0);
});
