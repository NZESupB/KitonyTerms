//! 局域网分享配对二维码。
//!
//! 只做编码 + 内联 SVG 渲染：`qrcode` 的 image/svg 后端会拖进图像编码依赖，而
//! WebView 直接吃 SVG，把模块矩阵拼成 path 是最省依赖的做法。

use dioxus::prelude::*;
use qrcode::{EcLevel, QrCode};

/// 二维码四周的静默区（模块数）。规范要求至少 4 个模块，否则扫码器难以定位。
const QUIET_ZONE: u32 = 4;

/// 把内容编码为二维码，返回 `(SVG path 数据, 含静默区的边长模块数)`。
///
/// 用一条 path 承载所有深色模块（每个模块一个 `M x y h1 v1 h-1 z` 子路径），
/// 渲染时按 `viewBox` 缩放，避免逐模块生成上千个 DOM 节点。
pub fn qr_svg_path(data: &str) -> Option<(String, u32)> {
    // 配对信息很短，纠错等级取 M：容错够用且模块数不会膨胀。
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M).ok()?;
    let width = u32::try_from(code.width()).ok()?;
    let modules = code.to_colors();
    let mut path = String::new();
    for (index, color) in modules.iter().enumerate() {
        if !matches!(color, qrcode::Color::Dark) {
            continue;
        }
        let index = u32::try_from(index).ok()?;
        let x = index % width + QUIET_ZONE;
        let y = index / width + QUIET_ZONE;
        // 单个模块的 1x1 方块。
        path.push_str(&format!("M{x} {y}h1v1h-1z"));
    }
    (!path.is_empty()).then_some((path, width + QUIET_ZONE * 2))
}

/// 配对二维码。深色模块用 `currentColor`，浅底固定为白色——扫码器依赖高对比度，
/// 跟随主题反色反而会降低识别率。
#[component]
pub fn QrCodeView(data: String, label: String) -> Element {
    let Some((path, size)) = qr_svg_path(&data) else {
        return rsx! {};
    };

    rsx! {
        svg {
            class: "qr-code",
            role: "img",
            "aria-label": "{label}",
            "viewBox": "0 0 {size} {size}",
            "shape-rendering": "crispEdges",
            rect {
                width: "{size}",
                height: "{size}",
                fill: "#ffffff",
            }
            path { d: "{path}", fill: "#000000" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_pairing_payload_with_quiet_zone() {
        let (path, size) = qr_svg_path(
            "kitonyterms://lan-share?v=2&url=http://192.168.1.20:12345/v2/config&code=0123456789ABCDEFGHJKMNPQRS",
        )
        .unwrap();
        // 最小的 21x21 版本加上两侧静默区。
        assert!(size >= 21 + QUIET_ZONE * 2);
        assert!(path.starts_with('M'));
        // 每个深色模块都是一个闭合子路径。
        assert_eq!(path.matches('M').count(), path.matches('z').count());
    }

    #[test]
    fn dark_modules_never_land_in_the_quiet_zone() {
        let (path, size) = qr_svg_path("0123456789ABCDEFGHJKMNPQRS").unwrap();
        let coordinates = path
            .split('M')
            .filter(|segment| !segment.is_empty())
            .map(|segment| {
                let head = segment.split('h').next().unwrap_or_default();
                let mut parts = head.split_whitespace();
                let x: u32 = parts.next().unwrap().parse().unwrap();
                let y: u32 = parts.next().unwrap().parse().unwrap();
                (x, y)
            })
            .collect::<Vec<_>>();
        assert!(!coordinates.is_empty());
        for (x, y) in coordinates {
            assert!(x >= QUIET_ZONE && y >= QUIET_ZONE);
            assert!(x < size - QUIET_ZONE && y < size - QUIET_ZONE);
        }
    }

    #[test]
    fn empty_input_still_encodes_a_valid_symbol() {
        // 空字符串是合法的二维码内容，不应 panic。
        assert!(qr_svg_path("").is_some());
    }
}
