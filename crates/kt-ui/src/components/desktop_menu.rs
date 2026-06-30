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

    menu.append_items(&[&app_menu, &settings_menu])
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
