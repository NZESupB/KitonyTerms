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

#[cfg(test)]
mod tests {
    use super::*;
    use kt_config::{DEFAULT_DARK_THEME, DEFAULT_LIGHT_THEME, DEFAULT_SYSTEM_THEME};

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
}
