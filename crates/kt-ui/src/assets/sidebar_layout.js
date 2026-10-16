window.__ktSidebarLayoutCleanup?.();
const content = document.querySelector('.kt-content');
if (content) {
    const dock = content.querySelector('.operations-dock');
    const splitter = content.querySelector('#sidebar-width-splitter');
    let last = '';
    const report = () => {
        const sizes = [
            content.clientWidth,
            dock?.getBoundingClientRect().width || 0,
            splitter?.getBoundingClientRect().width || 0,
        ];
        const signature = sizes.join(',');
        if (signature !== last) {
            last = signature;
            dioxus.send(sizes);
        }
    };
    const blur = () => dioxus.send([]);
    let finish;
    const finished = new Promise(resolve => { finish = resolve; });
    const observer = new ResizeObserver(report);
    observer.observe(content);
    if (dock) observer.observe(dock);
    if (splitter) observer.observe(splitter);
    window.addEventListener('resize', report);
    window.addEventListener('blur', blur);
    window.__ktSidebarLayoutCleanup = () => {
        observer.disconnect();
        window.removeEventListener('resize', report);
        window.removeEventListener('blur', blur);
        delete window.__ktSidebarLayoutCleanup;
        finish();
    };
    report();
    await finished;
}
