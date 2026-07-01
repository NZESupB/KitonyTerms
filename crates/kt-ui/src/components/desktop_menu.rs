//! 原生桌面菜单集成。

pub const SETTINGS_MENU_ID: &str = "kitonyterms-settings";

pub fn is_settings_menu_id(id: &str) -> bool {
    id == SETTINGS_MENU_ID
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn app_menu() -> dioxus::desktop::muda::Menu {
    use dioxus::desktop::muda::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    let menu = Menu::new();
    let app_menu = Submenu::new("KitonyTerms", true);
    let settings_menu = Submenu::new("设置", true);
    let settings = MenuItem::with_id(SETTINGS_MENU_ID, "设置...", true, None);

    app_menu
        .append_items(&[
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(None),
        ])
        .expect("无法创建应用菜单");
    settings_menu.append(&settings).expect("无法创建设置菜单");

    // 编辑菜单：macOS WebView(wry/tao) 依赖原生菜单的快捷键等价项来路由
    // Cmd+X/C/V/A、撤销/重做到聚焦的 <input>；缺少该菜单时输入框无法粘贴。
    // Edit menu: on macOS the WebView relies on these predefined items' key
    // equivalents to route Cut/Copy/Paste/SelectAll/Undo/Redo into the focused
    // <input>. Without it, pasting into text fields silently fails.
    let edit_menu = Submenu::new("编辑", true);
    edit_menu
        .append_items(&[
            &PredefinedMenuItem::undo(None),
            &PredefinedMenuItem::redo(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::cut(None),
            &PredefinedMenuItem::copy(None),
            &PredefinedMenuItem::paste(None),
            &PredefinedMenuItem::select_all(None),
        ])
        .expect("无法创建编辑菜单");

    menu.append_items(&[&app_menu, &edit_menu, &settings_menu])
        .expect("无法创建原生菜单");
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_menu_id_is_stable() {
        assert!(is_settings_menu_id("kitonyterms-settings"));
        assert!(!is_settings_menu_id("other"));
    }
}
