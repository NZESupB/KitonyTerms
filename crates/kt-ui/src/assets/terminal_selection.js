// terminalId、bridgeKey 由 Rust 注入；每个实例只有一条有序请求链。
// Dioxus 在 eval 返回后关闭回复通道；必须保持到 cleanup 才结束。
return new Promise(resolve => {
    const bridges = window.__ktTerminalSelectionBridges ||= new Map();
    bridges.get(bridgeKey)?.cleanup();
    const surface = document.getElementById(terminalId);
    const screen = document.getElementById(`${terminalId}-screen`);
    if (!surface || !screen) { dioxus.send(null); resolve(); return; }
    surface.__ktSelection?.cleanup();
    const abort = new AbortController();
    let disposed = false;
    let tail = Promise.resolve();
    let drag = null;
    let frame = 0;
    let dirty = false;
    let pendingMove = false;
    let wheelLines = 0;
    let lastAuto = 0;
    let nativeRange = null;
    let nativeTracking = false;
    let nativeRevision = 0;
    let fallbackCopying = false;
    let clicks = { time: -Infinity, x: 0, y: 0, count: 0 };

    const on = (target, name, handler, options = {}) =>
        target.addEventListener(name, handler, { ...options, signal: abort.signal });
    const report = error => {
        if (!disposed) console.error("终端选区操作失败", error.message || error);
    };
    const stamp = () => ({
        revision: Number(screen.dataset.revision),
        display_offset: Number(screen.dataset.displayOffset),
    });
    const hit = location => {
        const rowElement = screen.querySelector('.terminal-row');
        const cell = rowElement?.querySelector('span[data-col]:not([data-spacer])');
        const rows = Number(screen.dataset.rows);
        const cols = Number(screen.dataset.cols);
        const rowHeight = rowElement?.getBoundingClientRect().height;
        const cellWidth = cell?.getBoundingClientRect().width / Number(cell.dataset.columns);
        if (!rows || !cols || !rowHeight || !cellWidth) throw new Error("终端几何信息不可用");
        const rect = screen.getBoundingClientRect();
        const x = Math.max(0, Math.min(cols, (location.x - rect.left) / cellWidth));
        const column = Math.min(cols - 1, Math.floor(x));
        return {
            row: Math.max(0, Math.min(rows - 1, Math.floor((location.y - rect.top) / rowHeight))),
            column,
            right: x - column >= 0.5,
            ...stamp(),
        };
    };
    const waitFrame = revision => new Promise((resolve, reject) => {
        const started = performance.now();
        const check = () => {
            if (disposed || !surface.isConnected) return reject(new Error("终端实例已卸载"));
            if (Number(screen.dataset.revision) >= revision) return resolve();
            if (performance.now() - started > 3000) return reject(new Error("等待终端新帧超时"));
            requestAnimationFrame(check);
        };
        check();
    });
    const writeWebClipboard = async text => {
        if (!text) return;
        if (navigator.clipboard?.writeText) {
            try { await navigator.clipboard.writeText(text); return; } catch (_) { /* 使用兼容写入。 */ }
        }
        const input = document.createElement('textarea');
        const active = document.activeElement;
        const selection = window.getSelection();
        const saved = selection?.rangeCount ? selection.getRangeAt(0).cloneRange() : null;
        input.value = text;
        input.style.cssText = 'position:fixed;left:0;top:0;opacity:0;pointer-events:none';
        document.body.append(input);
        fallbackCopying = true;
        try {
            input.select();
            if (!document.execCommand('copy')) throw new Error("剪贴板写入被拒绝");
        } finally {
            input.remove();
            fallbackCopying = false;
            if (saved?.commonAncestorContainer.isConnected) {
                selection.removeAllRanges();
                selection.addRange(saved);
            }
            if (active?.isConnected) active.focus({ preventScroll: true });
        }
    };
    const enqueue = (build, restart = false) => {
        if (restart) tail = tail.catch(() => {});
        const job = tail.then(async () => {
            for (let attempt = 0; attempt < 8; attempt++) {
                if (disposed) throw new Error("终端实例已卸载");
                const action = build(attempt);
                dioxus.send(action);
                const result = await dioxus.recv();
                if (disposed) throw new Error("终端实例已卸载");
                if (result.Err !== undefined) throw new Error(result.Err);
                const reply = result.Ok;
                if (!reply.accepted) {
                    // 原生选区保留的是原帧的端点，不能把它重贴到新输出上。
                    if (action.type === 'range') throw new Error("选区对应的内容已变化，请重新选择");
                    await waitFrame(reply.revision);
                    continue;
                }
                if (action.scroll) await waitFrame(reply.revision);
                if (reply.text !== null) await writeWebClipboard(reply.text);
                return reply;
            }
            throw new Error("终端持续刷新，无法确认选区坐标，请重新选择");
        });
        tail = job;
        job.catch(report);
        return job;
    };
    const removeNativeSelection = () => {
        const selection = window.getSelection();
        if (selection?.anchorNode && screen.contains(selection.anchorNode)) selection.removeAllRanges();
        nativeRange = null;
        nativeTracking = false;
    };
    const flushMove = () => {
        if (!drag || (!dirty && !wheelLines)) return;
        const location = { x: drag.x, y: drag.y };
        const scroll = wheelLines;
        const initialPoint = hit(location);
        const currentDrag = drag;
        dirty = false;
        wheelLines = 0;
        pendingMove = true;
        enqueue(attempt => ({ type: 'extend', point: attempt ? hit(location) : initialPoint, scroll })).then(
            () => { pendingMove = false; },
            () => { pendingMove = false; if (drag === currentDrag) stopDrag(false); },
        );
    };
    const tick = now => {
        frame = 0;
        if (!drag || disposed) return;
        const rect = surface.getBoundingClientRect();
        if (!pendingMove && now - lastAuto >= 60) {
            const edge = Math.min(24, rect.height / 4);
            const up = rect.top + edge - drag.y;
            const down = drag.y - rect.bottom + edge;
            if (up > 0 || down > 0) {
                wheelLines = up > 0 ? Math.min(4, Math.ceil(up / edge)) : -Math.min(4, Math.ceil(down / edge));
                lastAuto = now;
            }
            const dx = drag.x < rect.left + edge ? -20 : drag.x > rect.right - edge ? 20 : 0;
            if (dx) { surface.scrollLeft += dx; dirty = true; }
        }
        if (!pendingMove) flushMove();
        frame = requestAnimationFrame(tick);
    };
    const stopDrag = flush => {
        if (!drag) return;
        if (flush) flushMove();
        const pointerId = drag.id;
        drag = null;
        cancelAnimationFrame(frame);
        frame = 0;
        if (surface.hasPointerCapture(pointerId)) surface.releasePointerCapture(pointerId);
    };
    on(surface, 'pointerdown', event => {
        if (!screen.contains(event.target)) return;
        if (event.pointerType !== 'mouse') {
            surface.classList.remove('is-mouse-selection');
            nativeTracking = true;
            nativeRevision = stamp().revision;
            nativeRange = null;
            return;
        }
        if (event.button !== 0) return;
        event.preventDefault();
        stopDrag(true);
        removeNativeSelection();
        surface.classList.add('is-mouse-selection');
        surface.focus({ preventScroll: true });
        const location = { x: event.clientX, y: event.clientY };
        const continuous = event.timeStamp - clicks.time <= 500
            && Math.hypot(location.x - clicks.x, location.y - clicks.y) < 5;
        clicks = { ...location, time: event.timeStamp, count: continuous ? clicks.count % 3 + 1 : 1 };
        const mode = clicks.count === 3 ? 'lines' : clicks.count === 2 ? 'semantic' : 'simple';
        const initialPoint = hit(location);
        enqueue(attempt => ({ type: 'begin', point: attempt ? hit(location) : initialPoint, mode }), true);
        drag = { ...location, id: event.pointerId };
        dirty = false;
        wheelLines = 0;
        surface.setPointerCapture(event.pointerId);
        frame = requestAnimationFrame(tick);
    });
    on(surface, 'pointermove', event => {
        if (!drag || event.pointerId !== drag.id) return;
        if (Math.hypot(event.clientX - clicks.x, event.clientY - clicks.y) >= 5) clicks.count = 0;
        drag.x = event.clientX;
        drag.y = event.clientY;
        dirty = true;
    });
    on(surface, 'pointerup', event => {
        if (event.pointerId !== drag?.id) return;
        drag.x = event.clientX;
        drag.y = event.clientY;
        dirty = true;
        stopDrag(true);
    });
    on(surface, 'pointercancel', () => stopDrag(false));
    on(surface, 'lostpointercapture', () => stopDrag(true));
    on(window, 'blur', () => stopDrag(true));
    on(surface, 'wheel', event => {
        if (!drag || Math.abs(event.deltaX) > Math.abs(event.deltaY)) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        if (event.deltaY) wheelLines += event.deltaY < 0 ? 3 : -3;
        wheelLines = Math.max(-12, Math.min(12, wheelLines));
        dirty = true;
    }, { capture: true, passive: false });

    // 原生触摸选区转换成单元格边界，不读取 Selection.toString()。
    const boundary = (node, offset) => {
        let element = node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
        if (!screen.contains(element)) return null;
        if (node.nodeType === Node.ELEMENT_NODE) {
            const next = node.childNodes[offset];
            if (next) { element = next.nodeType === Node.ELEMENT_NODE ? next : next.parentElement; offset = 0; }
            else if (node.lastChild) {
                const last = node.lastChild;
                element = last.nodeType === Node.ELEMENT_NODE ? last : last.parentElement;
                offset = 1;
            }
        }
        const row = element.closest('.terminal-row');
        const cell = element.closest('span[data-col]') || (offset ? row?.lastElementChild : row?.firstElementChild);
        if (!row || !cell) return null;
        return {
            row: Number(row.dataset.row),
            column: Number(cell.dataset.col) + (offset ? Number(cell.dataset.columns || 1) - 1 : 0),
            right: offset > 0,
            ...stamp(),
        };
    };
    on(document, 'selectionchange', () => {
        if (!nativeTracking || drag || fallbackCopying || stamp().revision !== nativeRevision) return;
        const selection = window.getSelection();
        if (!selection || selection.isCollapsed || !selection.rangeCount) { nativeRange = null; return; }
        const range = selection.getRangeAt(0);
        const anchor = boundary(range.startContainer, range.startOffset);
        const focus = boundary(range.endContainer, range.endOffset);
        if (anchor && focus) nativeRange = { anchor, focus };
    });
    const copy = () => {
        flushMove();
        const restoreFocus = surface.contains(document.activeElement) || document.activeElement === document.body;
        if (nativeRange) {
            const range = nativeRange;
            enqueue(() => ({ type: 'range', ...range }), true);
        }
        return enqueue(() => ({ type: 'copy' })).then(reply => {
            const active = document.activeElement;
            if (!disposed && restoreFocus && (active === document.body || surface.contains(active))) {
                surface.focus({ preventScroll: true });
            }
            return reply;
        });
    };
    const selectAll = () => {
        stopDrag(true);
        removeNativeSelection();
        surface.classList.add('is-mouse-selection');
        surface.focus({ preventScroll: true });
        const point = { row: 0, column: 0, right: false, ...stamp() };
        return enqueue(attempt => ({ type: 'all', point: attempt ? { ...point, ...stamp() } : point }), true);
    };
    on(document, 'copy', event => {
        if (fallbackCopying) return;
        const selection = window.getSelection();
        if (selection && !selection.isCollapsed && !screen.contains(selection.anchorNode)) return;
        if (document.activeElement !== surface && !screen.contains(selection?.anchorNode)) return;
        event.preventDefault();
        copy().catch(() => {});
    }, { capture: true });
    on(surface, 'keydown', event => {
        const mac = /Mac|iPhone|iPad/.test(navigator.platform);
        const shortcut = mac ? event.metaKey && !event.ctrlKey : event.ctrlKey && event.shiftKey;
        if (!shortcut || event.altKey) return;
        const key = event.key.toLowerCase();
        if (key !== 'c' && key !== 'a') return;
        event.preventDefault();
        event.stopImmediatePropagation();
        (key === 'c' ? copy() : selectAll()).catch(() => {});
    }, { capture: true });
    const observer = new MutationObserver(() => { if (drag) dirty = true; });
    observer.observe(screen, { attributes: true, attributeFilter: ['data-display-offset', 'data-cols', 'data-rows'] });
    const controller = {
        copy,
        selectAll,
        cleanup() {
            if (disposed) return;
            disposed = true;
            stopDrag(false);
            abort.abort();
            observer.disconnect();
            surface.classList.remove('is-mouse-selection');
            if (surface.__ktSelection === controller) delete surface.__ktSelection;
            if (bridges.get(bridgeKey) === controller) bridges.delete(bridgeKey);
            dioxus.send(null);
            resolve();
        },
    };
    surface.__ktSelection = controller;
    bridges.set(bridgeKey, controller);
});
