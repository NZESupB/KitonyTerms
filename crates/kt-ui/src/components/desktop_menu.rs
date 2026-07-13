//! 原生桌面菜单集成。

use kt_config::AppLanguage;

use crate::i18n::texts;

pub const SETTINGS_MENU_ID: &str = "kitonyterms-settings";

pub fn is_settings_menu_id(id: &str) -> bool {
    id == SETTINGS_MENU_ID
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn app_menu(language: AppLanguage) -> dioxus::desktop::muda::Menu {
    use dioxus::desktop::muda::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    let (edit_label, settings_label) = desktop_menu_labels(language);
    let menu = Menu::new();
    let app_menu = Submenu::new("KitonyTerms", true);
    let settings_menu = Submenu::new(settings_label, true);
    let settings = MenuItem::with_id(SETTINGS_MENU_ID, settings_label, true, None);

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

    // Windows/macOS/Linux 统一覆盖 Dioxus 默认 Window/Edit 菜单，并保留编辑快捷键
    // 等价项，确保 WebView 聚焦输入框能正确处理撤销、复制、粘贴和全选。
    let edit_menu = Submenu::new(edit_label, true);
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

fn desktop_menu_labels(language: AppLanguage) -> (&'static str, &'static str) {
    let app = texts(language).app;
    (app.edit, app.settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_menu_id_is_stable() {
        assert!(is_settings_menu_id("kitonyterms-settings"));
        assert!(!is_settings_menu_id("other"));
    }

    #[test]
    fn desktop_menu_labels_follow_selected_language() {
        assert_eq!(desktop_menu_labels(AppLanguage::Chinese), ("编辑", "设置"));
        assert_eq!(
            desktop_menu_labels(AppLanguage::English),
            ("Edit", "Settings")
        );
    }
}
