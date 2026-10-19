window.__ktOperationsViewportCleanup?.();
const viewport = document.querySelector('.operations-drawer-body');
if (viewport) {
    let frame = 0;
    let finish;
    const finished = new Promise(resolve => { finish = resolve; });
    const report = () => {
        cancelAnimationFrame(frame);
        frame = 0;
        dioxus.send(viewport.scrollTop);
    };
    const scroll = () => {
        if (!frame) frame = requestAnimationFrame(report);
    };
    const remember = event => {
        const button = event.target.closest('.operations-action-button');
        if (button) {
            window.__ktOperationsReturnAction = button.title;
            report();
        }
    };
    viewport.scrollTop = restoreTop;
    if (restoreList) requestAnimationFrame(() => {
        const selected = document.querySelector('.operations-page-marker')?.dataset.selected;
        const rows = [...viewport.querySelectorAll('[data-resource-id]')];
        const row = rows.find(row => row.dataset.resourceId === selected);
        const actions = row ? [...row.querySelectorAll('button')] : [];
        const target = actions.find(button => button.title === window.__ktOperationsReturnAction)
            || actions[0] || viewport.querySelector('.operations-filter-input, .operations-tab');
        target?.focus({ preventScroll: true });
    });
    viewport.addEventListener('scroll', scroll, { passive: true });
    viewport.addEventListener('click', remember, true);
    window.__ktOperationsViewportCleanup = () => {
        cancelAnimationFrame(frame);
        viewport.removeEventListener('scroll', scroll);
        viewport.removeEventListener('click', remember, true);
        delete window.__ktOperationsViewportCleanup;
        finish();
    };
    await finished;
}
