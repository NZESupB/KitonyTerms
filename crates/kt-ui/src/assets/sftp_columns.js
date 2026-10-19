window.__ktSftpColumnsCleanup ??= new Map();
window.__ktSftpColumnsCleanup.get(columnId)?.();
const root = document.getElementById(columnId);
if (root) {
    const lifetime = new AbortController();
    const options = { signal: lifetime.signal };
    const handles = [...root.querySelectorAll('[data-column-resize]')];
    const minima = root.dataset.columnMin.split(',').map(Number);
    let frame = 0;
    let drag = null;
    let finishLifetime;
    const finished = new Promise(resolve => { finishLifetime = resolve; });
    const measured = () => {
        const header = root.querySelector('.sftp-table-head-row');
        const values = getComputedStyle(header).gridTemplateColumns.split(/\s+/).map(parseFloat);
        return values.length === 6 && values.every(Number.isFinite)
            ? values.map((v, i) => Math.min(2048, Math.max(minima[i], v))) : [...minima];
    };
    const updateAria = widths => handles.forEach((handle, index) => {
        handle.setAttribute('aria-valuenow', String(Math.round(widths[index])));
    });
    const paint = widths => {
        root.style.setProperty('--sftp-cols', widths.map(w => `${w}px`).join(' '));
        root.style.setProperty('--sftp-table-min', `${widths.reduce((a, b) => a + b, 32)}px`);
        updateAria(widths);
    };
    const finish = save => {
        if (!drag) return;
        const previous = drag;
        drag = null;
        cancelAnimationFrame(frame);
        frame = 0;
        if (save) {
            paint(previous.widths);
            dioxus.send(previous.widths.map(Math.round));
        } else {
            root.style.setProperty('--sftp-cols', previous.template);
            root.style.setProperty('--sftp-table-min', previous.minimum);
            updateAria(measured());
        }
        root.classList.remove('is-column-resizing');
        if (previous.handle.hasPointerCapture(previous.pointer)) {
            previous.handle.releasePointerCapture(previous.pointer);
        }
    };
    root.addEventListener('pointerdown', event => {
        const handle = event.target.closest('[data-column-resize]');
        if (!handle || !root.contains(handle) || event.button !== 0 || drag) return;
        const index = Number(handle.dataset.columnResize);
        if (!Number.isInteger(index) || index < 0 || index >= 6) return;
        event.preventDefault();
        event.stopPropagation();
        const before = measured();
        drag = { handle, index, pointer: event.pointerId, x: event.clientX,
            before, widths: [...before],
            template: root.style.getPropertyValue('--sftp-cols'),
            minimum: root.style.getPropertyValue('--sftp-table-min') };
        handle.focus({ preventScroll: true });
        handle.setPointerCapture(event.pointerId);
        root.classList.add('is-column-resizing');
    }, options);
    root.addEventListener('pointermove', event => {
        if (!drag || drag.pointer !== event.pointerId) return;
        event.preventDefault();
        event.stopPropagation();
        drag.widths[drag.index] = Math.min(2048, Math.max(minima[drag.index],
            drag.before[drag.index] + event.clientX - drag.x));
        if (!frame) frame = requestAnimationFrame(() => {
            frame = 0;
            if (drag) paint(drag.widths);
        });
    }, options);
    root.addEventListener('pointerup', event => {
        if (drag?.pointer === event.pointerId) {
            event.stopPropagation();
            finish(true);
        }
    }, options);
    const cancelPointer = event => {
        if (drag?.pointer === event.pointerId) finish(false);
    };
    root.addEventListener('pointercancel', cancelPointer, options);
    root.addEventListener('lostpointercapture', cancelPointer, options);
    root.addEventListener('dblclick', event => {
        if (!event.target.closest('[data-column-resize]')) return;
        event.preventDefault();
        event.stopPropagation();
        finish(false);
        dioxus.send([]);
    }, options);
    root.addEventListener('keydown', event => {
        if (event.key === 'Escape' && drag) {
            event.preventDefault();
            event.stopPropagation();
            finish(false);
            return;
        }
        const handle = event.target.closest('[data-column-resize]');
        if (!handle || !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
        event.preventDefault();
        event.stopPropagation();
        const index = Number(handle.dataset.columnResize);
        const widths = measured();
        const delta = (event.shiftKey ? 32 : 8) * (event.key === 'ArrowLeft' ? -1 : 1);
        widths[index] = event.key === 'Home' ? minima[index] : event.key === 'End' ? 2048
            : Math.min(2048, Math.max(minima[index], widths[index] + delta));
        paint(widths);
        dioxus.send(widths.map(Math.round));
    }, options);
    window.addEventListener('blur', () => finish(false), options);
    const observer = new ResizeObserver(() => { if (!drag) updateAria(measured()); });
    observer.observe(root);
    updateAria(measured());
    root.__ktColumnsCleanup = () => {
        finish(false);
        lifetime.abort();
        observer.disconnect();
        cancelAnimationFrame(frame);
        delete root.__ktColumnsCleanup;
        window.__ktSftpColumnsCleanup.delete(columnId);
        finishLifetime();
    };
    window.__ktSftpColumnsCleanup.set(columnId, root.__ktColumnsCleanup);
    await finished;
}
