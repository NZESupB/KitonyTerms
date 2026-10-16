// terminalId 与 restoreOnly 由 Rust 调用方提供；恢复焦点不会打断其他输入区域。
const focus = () => {
    const terminal = document.getElementById(terminalId);
    if (!terminal || terminal.closest('[inert]')) return;
    const visible = (element) => element.getClientRects().length > 0;
    if (!visible(terminal)) return;
    const overlays = document.querySelectorAll(
        '.settings-overlay, .inline-editor-overlay, [role="dialog"], dialog[open]'
    );
    if (Array.from(overlays).some(visible)) return;
    const active = document.activeElement;
    if (restoreOnly && active && active !== document.body && active !== document.documentElement
        && !active.matches('.terminal-surface, .terminal-placeholder')) return;
    terminal.focus({ preventScroll: true });
};
if (restoreOnly) requestAnimationFrame(focus);
else focus();
