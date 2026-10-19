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

/// 界面字体族：未配置或清洗后为空时返回 `None`，由 CSS 使用平台默认字体栈。
pub(crate) fn css_ui_font_family(value: Option<&str>) -> Option<String> {
    let sanitized = sanitize_font_family(value?);
    (!sanitized.trim().is_empty()).then_some(sanitized)
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
    fn ui_font_family_is_optional_and_sanitized() {
        // 未配置：交给 CSS 默认字体栈，不能落成空字符串。
        assert_eq!(css_ui_font_family(None), None);
        assert_eq!(css_ui_font_family(Some("   ")), None);
        assert_eq!(css_ui_font_family(Some(";:{}")), None);

        assert_eq!(
            css_ui_font_family(Some("Fira Sans; color: red")),
            Some("Fira Sans color red".to_string())
        );
        assert_eq!(
            css_ui_font_family(Some(" 苹方 SC ")),
            Some(" 苹方 SC ".to_string())
        );
    }
}
