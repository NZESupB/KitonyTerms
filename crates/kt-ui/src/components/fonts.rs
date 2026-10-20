//! 本机字体发现与安全的 CSS family 处理。

use std::collections::BTreeSet;

const FALLBACK_FONTS: &[&str] = &[
    "Menlo",
    "SF Mono",
    "Consolas",
    "DejaVu Sans Mono",
    "Courier New",
    "Monaco",
    "monospace",
];

/// 读取桌面系统字体族名称，并把等宽字体排在前面。
///
/// 移动端不扫描宿主文件系统，返回有限回退列表；当前配置值由设置组件
/// 额外补入，因此历史自定义字体不会从选择器中消失。
pub fn detect_system_fonts() -> Vec<String> {
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();

        let mut families = database
            .faces()
            .filter_map(|face| face.families.first().map(|(family, _)| family.clone()))
            .collect::<BTreeSet<_>>();
        for fallback in FALLBACK_FONTS {
            families.insert((*fallback).to_string());
        }

        let mut fonts = families.into_iter().collect::<Vec<_>>();
        fonts.sort_by_key(|font| (!looks_monospace(font), font.to_ascii_lowercase()));
        fonts
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        FALLBACK_FONTS
            .iter()
            .map(|font| (*font).to_string())
            .collect()
    }
}

/// 判断字体族名是否可能是等宽字体，仅用于排序，不作为字体可用性的判断。
fn looks_monospace(font: &str) -> bool {
    let normalized = font.to_ascii_lowercase();
    normalized.contains("mono")
        || normalized.contains("code")
        || normalized.contains("courier")
        || normalized.contains("console")
        || normalized.contains("menlo")
        || normalized.contains("monaco")
}

/// 过滤掉可用于样式注入的字符，只保留字母数字与常见字体族符号。
fn sanitize_font_family(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_alphanumeric()
                || matches!(character, ' ' | ',' | '-' | '_' | '.' | '\'' | '"')
        })
        .collect()
}

/// 将用户配置转换为可放入 inline style 的等宽字体族表达式。
pub(crate) fn css_font_family(value: &str) -> String {
    let filtered = sanitize_font_family(value);
    if filtered.trim().is_empty() {
        "\"SF Mono\", \"Menlo\", \"Consolas\", monospace".to_string()
    } else {
        filtered
    }
}

/// 界面选择器保存单个字体族；用 CSS 字符串保留原名，并明确缺字时的平台回退。
pub(crate) fn css_ui_font_family(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    // 通用字体族是 CSS 关键字，加引号会把 monospace 等变成不存在的具体字体名。
    let mut family = if matches!(
        value,
        "serif"
            | "sans-serif"
            | "monospace"
            | "cursive"
            | "fantasy"
            | "system-ui"
            | "ui-serif"
            | "ui-sans-serif"
            | "ui-monospace"
            | "ui-rounded"
            | "math"
            | "fangsong"
    ) {
        value.to_string()
    } else {
        let mut quoted = String::with_capacity(value.len() + 2);
        quoted.push('"');
        for character in value.chars() {
            match character {
                '"' | '\\' => {
                    quoted.push('\\');
                    quoted.push(character);
                }
                '\0' => quoted.push('\u{fffd}'),
                c if c.is_control() => quoted.push_str(&format!("\\{:x} ", u32::from(c))),
                c => quoted.push(c),
            }
        }
        quoted.push('"');
        quoted
    };
    family.push_str(", var(--kt-ui-font-default)");
    Some(family)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_font_list_is_non_empty_and_sorted_with_monospace_first() {
        let fonts = detect_system_fonts();
        assert!(!fonts.is_empty());
        let first_non_mono = fonts.iter().position(|font| !looks_monospace(font));
        if let Some(index) = first_non_mono {
            assert!(fonts[..index].iter().all(|font| looks_monospace(font)));
        }
    }

    #[test]
    fn css_font_family_rejects_style_injection_and_keeps_fallback() {
        assert_eq!(
            css_font_family("Fira Code; color: red"),
            "Fira Code color red"
        );
        assert_eq!(css_font_family("苹方 SC"), "苹方 SC");
        assert!(css_font_family(";:{}").contains("monospace"));
    }

    #[test]
    fn ui_font_family_is_optional() {
        assert_eq!(css_ui_font_family(None), None);
        assert_eq!(css_ui_font_family(Some("   ")), None);
    }

    #[test]
    fn ui_font_family_preserves_names_and_platform_fallback() {
        for name in [
            "Fira Sans (UI)",
            "苹方 SC",
            "123 Font",
            "Font, Regular",
            "A;:{}",
        ] {
            assert_eq!(
                css_ui_font_family(Some(&format!(" {name} "))),
                Some(format!("\"{name}\", var(--kt-ui-font-default)"))
            );
        }
    }

    #[test]
    fn ui_font_family_escapes_css_strings_without_adding_declarations() {
        assert_eq!(
            css_ui_font_family(Some("A\"; color: red; /*\\Font\nB\0C")),
            Some(
                "\"A\\\"; color: red; /*\\\\Font\\a B\u{fffd}C\", var(--kt-ui-font-default)"
                    .to_string()
            )
        );
    }

    #[test]
    fn ui_font_family_keeps_generic_family_semantics() {
        assert_eq!(
            css_ui_font_family(Some("monospace")),
            Some("monospace, var(--kt-ui-font-default)".to_string())
        );
    }
}
