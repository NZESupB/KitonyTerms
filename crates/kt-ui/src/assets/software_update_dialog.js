// 更新模态框独立于主界面的 inert 区域；清理后恢复触发按钮的焦点。
(() => {
    if (window.ktUpdateDialogCleanup) return;
    const previous = document.activeElement;
    let dialog = null;
    let frame = 0;
    let stopped = false;
    const focusable = () => [...dialog.querySelectorAll('button:not(:disabled), a[href], [tabindex="0"]')]
        .filter(element => element.getClientRects().length && !element.closest('[inert]'));
    const focusInside = () => {
        const later = dialog.querySelector('#update-restart-later');
        (later || dialog).focus({ preventScroll: true });
    };
    const onKey = event => {
        if (!dialog?.isConnected) return;
        if (event.key === 'Escape') {
            event.preventDefault();
            event.stopImmediatePropagation();
            if (dialog.dataset.preparing !== 'true') dialog.querySelector('#update-restart-later')?.click();
        } else if (event.key === 'Tab') {
            event.preventDefault();
            event.stopImmediatePropagation();
            const elements = focusable();
            const index = elements.indexOf(document.activeElement);
            const next = event.shiftKey
                ? (index <= 0 ? elements.length - 1 : index - 1)
                : (index + 1) % elements.length;
            (elements[next] || dialog).focus({ preventScroll: true });
        }
    };
    const onFocus = event => {
        if (dialog?.isConnected && !dialog.contains(event.target)) focusInside();
    };
    const observer = new MutationObserver(() => {
        if (!dialog) return;
        if (!dialog.isConnected) {
            cleanup();
        } else if (!dialog.contains(document.activeElement)) {
            focusInside();
        }
    });
    const cleanup = () => {
        if (stopped) return;
        stopped = true;
        cancelAnimationFrame(frame);
        observer.disconnect();
        document.removeEventListener('keydown', onKey, true);
        document.removeEventListener('focusin', onFocus, true);
        delete window.ktUpdateDialogCleanup;
        requestAnimationFrame(() => {
            if (document.querySelector('#software-update-dialog')) return;
            const target = previous?.isConnected && previous !== document.body && previous.getClientRects().length && !previous.closest('[inert]')
                ? previous
                : document.querySelector('.software-update-notice:not([hidden]) .primary, .update-status-card .primary');
            target?.focus({ preventScroll: true });
        });
    };
    window.ktUpdateDialogCleanup = cleanup;
    let attempts = 0;
    const mount = () => {
        if (stopped) return;
        dialog = document.querySelector('#software-update-dialog');
        if (!dialog) {
            if (++attempts < 30) frame = requestAnimationFrame(mount);
            else cleanup();
            return;
        }
        document.addEventListener('keydown', onKey, true);
        document.addEventListener('focusin', onFocus, true);
        observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-preparing'] });
        focusInside();
    };
    frame = requestAnimationFrame(mount);
})();
