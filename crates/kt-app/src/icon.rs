//! 应用外部图标资产与平台接入。

const APP_ICON_PNG: &[u8] = include_bytes!("../assets/app-icon.png");

#[cfg(target_os = "macos")]
const APP_ICON_ICNS: &[u8] = include_bytes!("../assets/macos/KitonyTerms.icns");

pub fn kitony_window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    dioxus::desktop::icon_from_memory(APP_ICON_PNG).ok()
}

#[cfg(target_os = "macos")]
pub fn with_platform_icon_hooks(config: dioxus::desktop::Config) -> dioxus::desktop::Config {
    use dioxus::desktop::tao::event::Event;

    let mut dock_icon_applied = false;
    config.with_custom_event_handler(move |event, _| {
        if !dock_icon_applied && matches!(event, Event::MainEventsCleared) {
            if !set_macos_dock_icon() {
                tracing::warn!("macOS Dock 图标设置失败");
            }
            dock_icon_applied = true;
        }
    })
}

#[cfg(not(target_os = "macos"))]
pub fn with_platform_icon_hooks(config: dioxus::desktop::Config) -> dioxus::desktop::Config {
    config
}

#[cfg(target_os = "macos")]
pub fn set_macos_dock_icon() -> bool {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let data = NSData::with_bytes(APP_ICON_ICNS);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return false;
    };

    unsafe {
        NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
    }
    true
}

#[cfg(not(target_os = "macos"))]
pub fn set_macos_dock_icon() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_app_icon_is_png() {
        assert!(APP_ICON_PNG.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn bundled_platform_icons_have_known_headers() {
        let macos_icon = include_bytes!("../assets/macos/KitonyTerms.icns");
        let windows_icon = include_bytes!("../assets/windows/kitonyterms.ico");

        assert!(macos_icon.starts_with(b"icns"));
        assert!(windows_icon.starts_with(&[0, 0, 1, 0]));
    }

    #[test]
    fn kitony_window_icon_can_be_created() {
        assert!(kitony_window_icon().is_some());
    }
}
