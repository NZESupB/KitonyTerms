//! 应用主题解析和系统深浅模式监听。

use dioxus::prelude::*;
use kt_config::resolve_theme_name;

/// 返回当前系统是否为浅色模式。无法读取时保持 `false`，即深色回退。
pub fn use_system_theme() -> Signal<bool> {
    let mut system_is_light = use_signal(|| false);

    use_effect(move || {
        spawn(async move {
            let mut eval = dioxus::document::eval(
                r#"
                const media = window.matchMedia("(prefers-color-scheme: light)");
                const report = () => dioxus.send([Boolean(media.matches)]);
                const listener = () => report();
                if (media.addEventListener) {
                    media.addEventListener("change", listener);
                } else if (media.addListener) {
                    media.addListener(listener);
                }
                report();
                await new Promise(() => {});
                "#,
            );

            while let Ok(payload) = eval.recv::<Vec<bool>>().await {
                if let Some(is_light) = payload.first().copied() {
                    system_is_light.set(is_light);
                }
            }
        });
    });

    system_is_light
}

pub(crate) fn concrete_theme_name(theme: &str, system_is_light: bool) -> &'static str {
    resolve_theme_name(theme, system_is_light)
}

/// 两个渲染根（`.kt-window` 与 `.software-update-layer`）共用的内联变量。
///
/// 界面字体族与缩放只在这里写入，具体尺寸令牌定义在 `app.css` 中；更新弹层位于
/// `.kt-window` 之外，因此必须与主界面使用同一份拼装逻辑，避免两处漂移。
pub(crate) fn interface_root_style(
    accent: &str,
    accent_rgb: &str,
    ink: &str,
    ui_font: Option<&str>,
    ui_scale: f32,
) -> String {
    let mut style = format!(
        "--blue: {accent}; --accent-rgb: {accent_rgb}; --update-button-ink: {ink}; \
         --kt-ui-scale: {ui_scale:.3};"
    );
    if let Some(family) = ui_font {
        style.push_str(&format!(" --kt-ui-font: {family};"));
    }
    style
}

/// 在用户强调色上选择对比度更高的前景，供更新按钮和开关使用。
pub(crate) fn accent_foreground(accent: &str) -> &'static str {
    let rgb = u32::from_str_radix(accent.trim_start_matches('#'), 16).unwrap_or(0x5aa7ff);
    let linear = |shift: u32| {
        let value = ((rgb >> shift) & 255u32) as f64 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0);
    if (luminance + 0.05) / 0.0559 >= 1.05 / (luminance + 0.05) {
        "#08121f"
    } else {
        "#ffffff"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME, DEFAULT_SYSTEM_THEME};

    #[test]
    fn update_controls_choose_readable_accent_text() {
        assert_eq!(accent_foreground("#ffffff"), "#08121f");
        assert_eq!(accent_foreground("#ffff00"), "#08121f");
        assert_eq!(accent_foreground("#5aa7ff"), "#08121f");
        assert_eq!(accent_foreground("#2563eb"), "#ffffff");
        assert_eq!(accent_foreground("#000000"), "#ffffff");
    }

    #[test]
    fn system_preference_resolves_to_the_concrete_theme() {
        assert_eq!(
            concrete_theme_name(DEFAULT_SYSTEM_THEME, true),
            DEFAULT_LIGHT_THEME
        );
        assert_eq!(
            concrete_theme_name(DEFAULT_SYSTEM_THEME, false),
            DEFAULT_DARK_THEME
        );
        assert_eq!(
            concrete_theme_name(DEFAULT_LIGHT_THEME, false),
            DEFAULT_LIGHT_THEME
        );
    }

    #[test]
    fn interface_root_style_carries_accent_and_interface_font() {
        let without_font = interface_root_style("#5aa7ff", "90, 167, 255", "#08121f", None, 1.0);
        assert_eq!(
            without_font,
            "--blue: #5aa7ff; --accent-rgb: 90, 167, 255; --update-button-ink: #08121f; \
             --kt-ui-scale: 1.000;"
        );
        assert!(!without_font.contains("--kt-ui-font"));

        let scaled = interface_root_style(
            "#5aa7ff",
            "90, 167, 255",
            "#08121f",
            Some("Fira Sans"),
            1.25,
        );
        assert!(scaled.ends_with("--kt-ui-scale: 1.250; --kt-ui-font: Fira Sans;"));
    }
}
