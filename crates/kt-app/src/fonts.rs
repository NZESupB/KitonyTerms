//! 中文字体回退 —— egui 内置字体仅含拉丁字形,CJK 字符会显示为豆腐块(乱码)。
//! 这里在启动时探测系统已安装的 CJK 字体,并作为回退追加到比例字体与等宽字体族,
//! 既保留原有拉丁外观,又补全界面与终端的中文显示。
//!
//! CJK font fallback: egui's bundled fonts cover only Latin glyphs, so Chinese
//! text renders as tofu. We probe for a system-installed CJK font at startup and
//! append it as a fallback to both the proportional and monospace families.

use eframe::egui;

/// 各平台常见的 CJK 字体文件候选路径,按优先级排列。
/// Candidate CJK font files per platform, in priority order.
#[cfg(target_os = "macos")]
const CJK_FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
];

#[cfg(target_os = "windows")]
const CJK_FONT_CANDIDATES: &[&str] = &[
    r"C:\Windows\Fonts\msyh.ttc",   // 微软雅黑 Microsoft YaHei
    r"C:\Windows\Fonts\msyh.ttf",
    r"C:\Windows\Fonts\simhei.ttf", // 黑体 SimHei
    r"C:\Windows\Fonts\simsun.ttc", // 宋体 SimSun
];

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const CJK_FONT_CANDIDATES: &[&str] = &[
    // Noto Sans CJK(Debian/Ubuntu、Fedora、Arch 等常见安装路径)
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    // 文泉驿 WenQuanYi(轻量发行版常见)
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
];

/// 探测并安装 CJK 回退字体。找不到时仅记录告警,不影响启动。
/// Probe for and install a CJK fallback font. Logs a warning if none is found.
pub fn install_cjk_fallback(ctx: &egui::Context) {
    let Some((path, bytes)) = CJK_FONT_CANDIDATES
        .iter()
        .find_map(|p| std::fs::read(p).ok().map(|b| (*p, b)))
    else {
        tracing::warn!(
            "未找到系统 CJK 字体,界面中文可能显示为乱码;\
             候选路径均不存在 / no system CJK font found, Chinese text may render as tofu"
        );
        return;
    };

    const NAME: &str = "system-cjk";
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert(NAME.to_owned(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));

    // 作为回退追加到两个字体族末尾:拉丁仍用原字体,缺失字形(中文)再回落到 CJK。
    // Append as last-resort fallback: Latin keeps the original font, missing
    // glyphs (Chinese) fall through to the CJK font.
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(NAME.to_owned());
    }

    ctx.set_fonts(fonts);
    tracing::info!("已加载 CJK 回退字体 / loaded CJK fallback font: {path}");
}
