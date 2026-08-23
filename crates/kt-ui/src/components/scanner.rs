//! 摄像头扫描配对二维码。
//!
//! 走 WebView 的 \`getUserMedia\`：wry 0.53 的 \`RustWebChromeClient.onPermissionRequest\`
//! 已经把 \`VIDEO_CAPTURE\` 映射到 \`Manifest.permission.CAMERA\` 并驱动运行时授权流程，
//! 所以这条路不需要任何自定义 Java/Kotlin，只要在 \`Dioxus.toml\` 声明 CAMERA 权限。
//!
//! 解码放在 Rust（\`rqrr\`）而不是内联一份 JS 解码库：JS 侧只负责取帧并把灰度缓冲
//! 递过来，识别逻辑保持可单元测试。

use base64::{engine::general_purpose::STANDARD, Engine as _};
use dioxus::prelude::dioxus_core::Task;
use dioxus::prelude::*;

use crate::components::icons::Icon;
use crate::i18n::texts;
use kt_config::AppLanguage;

/// 摄像头取景的目标边长（像素）。缩到 480 足够识别配对二维码，同时把每帧要
/// 跨 IPC 传输和解码的数据量压到可接受范围。
const FRAME_SIDE: usize = 480;

/// 扫码是否可用。只有移动端有摄像头取景的使用场景；桌面端保持手动输入，
/// 避免为一个少用路径引入摄像头权限提示。
pub fn scan_supported() -> bool {
    cfg!(any(target_os = "android", target_os = "ios"))
}

/// 扫码结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanOutcome {
    /// 成功识别出内容，交由上层解析成地址与配对码。
    Decoded(String),
    /// 用户主动取消。
    Cancelled,
    /// 相机权限被拒绝。
    PermissionDenied,
    /// 当前环境没有可用摄像头或不支持取流。
    Unsupported,
}

/// 从灰度帧中识别二维码内容。
///
/// \`frame\` 是 \`width * height\` 的 8 位灰度像素，行优先。识别不到时返回 \`None\`，
/// 调用方继续送下一帧即可。
pub fn decode_greyscale_frame(frame: &[u8], width: usize, height: usize) -> Option<String> {
    let expected_len = width.checked_mul(height)?;
    if expected_len == 0 || frame.len() != expected_len {
        return None;
    }
    let mut image =
        rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| frame[y * width + x]);
    for grid in image.detect_grids() {
        if let Ok((_, content)) = grid.decode() {
            if !content.is_empty() {
                return Some(content);
            }
        }
    }
    None
}

/// 把 JS 侧回传的一帧解释为扫码结果。
///
/// 约定的消息形状：\`[tag, width, height, base64_pixels]\`。错误消息复用相同结构，
/// 宽高和数据留空，避免 Eval 接收端为不同消息维护多个 JSON 类型。
pub fn interpret_frame_message(
    tag: u8,
    width: usize,
    height: usize,
    encoded_pixels: &str,
) -> Option<ScanOutcome> {
    match tag {
        // 0 = 帧数据
        0 => {
            let expected_len = width.checked_mul(height)?;
            if expected_len == 0 || expected_len > FRAME_SIDE * FRAME_SIDE {
                return None;
            }
            let pixels = STANDARD.decode(encoded_pixels).ok()?;
            if pixels.len() != expected_len {
                return None;
            }
            decode_greyscale_frame(&pixels, width, height).map(ScanOutcome::Decoded)
        }
        // 1 = 权限被拒
        1 => Some(ScanOutcome::PermissionDenied),
        // 2 = 无可用摄像头
        2 => Some(ScanOutcome::Unsupported),
        // 3 = 用户取消
        3 => Some(ScanOutcome::Cancelled),
        _ => None,
    }
}

/// 取景 + 抽帧脚本。
///
/// 只做三件事：申请摄像头、把视频帧画到离屏 canvas、把灰度缓冲送回 Rust。识别与
/// 状态判断都在 Rust 侧完成。
fn scanner_script() -> String {
    format!(
        r#"
        const SIDE = {FRAME_SIDE};
        const FRAME_INTERVAL_MS = 125;
        const video = document.getElementById("kt-scan-video");
        const canvas = document.createElement("canvas");
        canvas.width = SIDE;
        canvas.height = SIDE;
        const context = canvas.getContext("2d", {{ willReadFrequently: true }});
        let stream = null;
        let stopped = false;
        let lastFrameAt = 0;

        const stop = () => {{
            stopped = true;
            if (stream) {{
                for (const track of stream.getTracks()) track.stop();
                stream = null;
            }}
        }};
        const stopCommand = dioxus.recv().catch(() => "stop");

        try {{
            if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {{
                dioxus.send([2, 0, 0, ""]);
            }} else {{
                // facingMode 后置优先：扫别人屏幕上的二维码。
                stream = await navigator.mediaDevices.getUserMedia({{
                    video: {{ facingMode: "environment" }},
                    audio: false,
                }});
                if (video) {{
                    video.srcObject = stream;
                    await video.play().catch(() => {{}});
                }}

                while (!stopped) {{
                    // 等一帧，避免忙等占满主线程。
                    const timestamp = await Promise.race([
                        new Promise((resolve) => requestAnimationFrame(resolve)),
                        stopCommand.then(() => null),
                    ]);
                    if (timestamp === null) break;
                    if (stopped || !video || video.readyState < 2) continue;
                    if (timestamp - lastFrameAt < FRAME_INTERVAL_MS) continue;
                    lastFrameAt = timestamp;

                    // 取中央正方形区域，等比缩放到 SIDE。
                    const side = Math.min(video.videoWidth, video.videoHeight);
                    if (!side) continue;
                    const sx = (video.videoWidth - side) / 2;
                    const sy = (video.videoHeight - side) / 2;
                    context.drawImage(video, sx, sy, side, side, 0, 0, SIDE, SIDE);
                    const data = context.getImageData(0, 0, SIDE, SIDE).data;

                    // RGBA → 灰度（BT.601 亮度权重）。
                    const frame = new Uint8Array(SIDE * SIDE);
                    for (let i = 0, p = 0; i < data.length; i += 4, p += 1) {{
                        frame[p] = (data[i] * 0.299 + data[i + 1] * 0.587 + data[i + 2] * 0.114) | 0;
                    }}
                    let binary = "";
                    const chunkSize = 0x8000;
                    for (let offset = 0; offset < frame.length; offset += chunkSize) {{
                        binary += String.fromCharCode(...frame.subarray(offset, offset + chunkSize));
                    }}
                    dioxus.send([0, SIDE, SIDE, btoa(binary)]);
                }}
            }}
        }} catch (error) {{
            const name = error && error.name ? error.name : "";
            // NotAllowedError = 用户拒绝授权；其余按设备不可用处理。
            dioxus.send([name === "NotAllowedError" || name === "SecurityError" ? 1 : 2, 0, 0, ""]);
        }} finally {{
            stop();
        }}
        return null;
        "#
    )
}

/// 全屏扫码取景层。
#[component]
pub fn PairingScanner(
    show: Signal<bool>,
    language: AppLanguage,
    on_result: EventHandler<ScanOutcome>,
) -> Element {
    let mut active_eval = use_signal(|| None::<dioxus::document::Eval>);
    let mut scan_task = use_signal(|| None::<Task>);

    use_drop(move || {
        if let Some(eval) = active_eval.take() {
            let _ = eval.send("stop");
        }
        if let Some(task) = scan_task.take() {
            task.cancel();
        }
    });

    use_effect(move || {
        let visible = show();
        if let Some(eval) = active_eval.take() {
            let _ = eval.send("stop");
        }
        if let Some(task) = scan_task.take() {
            task.cancel();
        }
        if !visible {
            return;
        }
        let eval = dioxus::document::eval(&scanner_script());
        active_eval.set(Some(eval));
        let task = spawn(async move {
            let mut receiver = eval;
            while let Ok((tag, width, height, pixels)) =
                receiver.recv::<(u8, usize, usize, String)>().await
            {
                if let Some(outcome) = interpret_frame_message(tag, width, height, &pixels) {
                    let _ = receiver.send("stop");
                    on_result.call(outcome);
                    break;
                }
            }
        });
        scan_task.set(Some(task));
    });

    let visible = show();

    if !visible {
        return rsx! {};
    }

    let t = texts(language).app;

    rsx! {
        div {
            class: "scanner-overlay",
            div {
                class: "scanner-head",
                h2 { "{t.sync_scan}" }
                button {
                    class: "icon-button slim",
                    title: "{t.close}",
                    onclick: move |_| on_result.call(ScanOutcome::Cancelled),
                    Icon { name: "close" }
                }
            }
            div {
                class: "scanner-viewport",
                video {
                    id: "kt-scan-video",
                    class: "scanner-video",
                    autoplay: true,
                    muted: true,
                    playsinline: true,
                }
                div { class: "scanner-reticle" }
            }
            p { class: "scanner-hint", "{t.sync_scan_hint}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::qr::qr_svg_path;

    /// 用编码器生成的模块矩阵渲染成灰度位图，验证解码端能读回同样的内容。
    fn render_greyscale(data: &str, scale: usize) -> (Vec<u8>, usize) {
        let code = qrcode::QrCode::with_error_correction_level(data.as_bytes(), qrcode::EcLevel::M)
            .unwrap();
        let width = code.width();
        let quiet = 4;
        let side = (width + quiet * 2) * scale;
        let colors = code.to_colors();
        let mut buffer = vec![255u8; side * side];
        for y in 0..width {
            for x in 0..width {
                if !matches!(colors[y * width + x], qrcode::Color::Dark) {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = (x + quiet) * scale + dx;
                        let py = (y + quiet) * scale + dy;
                        buffer[py * side + px] = 0;
                    }
                }
            }
        }
        (buffer, side)
    }

    #[test]
    fn decodes_a_rendered_pairing_payload() {
        let payload = "kitonyterms://lan-share?v=2&url=http://192.168.1.20:12345/v2/config&code=0123456789ABCDEFGHJKMNPQRS";
        let (buffer, side) = render_greyscale(payload, 4);
        assert_eq!(
            decode_greyscale_frame(&buffer, side, side).as_deref(),
            Some(payload)
        );
        // 编码器与解码器对同一负载必须都能处理。
        assert!(qr_svg_path(payload).is_some());
    }

    #[test]
    fn blank_and_malformed_frames_decode_to_nothing() {
        assert!(decode_greyscale_frame(&[255; 64 * 64], 64, 64).is_none());
        assert!(decode_greyscale_frame(&[], 0, 0).is_none());
        // 缓冲比声明的尺寸小时不应 panic。
        assert!(decode_greyscale_frame(&[0; 10], 64, 64).is_none());
    }

    #[test]
    fn frame_messages_map_to_outcomes() {
        assert_eq!(
            interpret_frame_message(1, 0, 0, ""),
            Some(ScanOutcome::PermissionDenied)
        );
        assert_eq!(
            interpret_frame_message(2, 0, 0, ""),
            Some(ScanOutcome::Unsupported)
        );
        assert_eq!(
            interpret_frame_message(3, 0, 0, ""),
            Some(ScanOutcome::Cancelled)
        );
        assert!(interpret_frame_message(9, 0, 0, "").is_none());
    }

    #[test]
    fn a_frame_message_carrying_a_code_is_decoded() {
        let code = "0123456789ABCDEFGHJKMNPQRS";
        let (buffer, side) = render_greyscale(code, 4);
        let payload = STANDARD.encode(buffer);
        assert_eq!(
            interpret_frame_message(0, side, side, &payload),
            Some(ScanOutcome::Decoded(code.to_string()))
        );
    }

    #[test]
    fn undecodable_frames_keep_the_scanner_running() {
        // 纯白帧不应结束扫码流程。
        let payload = STANDARD.encode([255; 64 * 64]);
        assert!(interpret_frame_message(0, 64, 64, &payload).is_none());
    }

    #[test]
    fn malformed_frame_payloads_are_rejected() {
        assert!(interpret_frame_message(0, 64, 64, "not base64").is_none());
        let short = STANDARD.encode([0; 10]);
        assert!(interpret_frame_message(0, 64, 64, &short).is_none());
        assert!(interpret_frame_message(0, FRAME_SIDE + 1, FRAME_SIDE, "").is_none());
    }

    #[test]
    fn scanner_script_is_throttled_and_has_explicit_cleanup() {
        let script = scanner_script();
        assert!(script.contains("FRAME_INTERVAL_MS = 125"));
        assert!(script.contains("dioxus.recv()"));
        assert!(script.contains("track.stop()"));
        assert!(script.contains("finally"));
        assert!(!script.contains("...frame]"));
        assert!(!script.contains("new Promise(() => {})"));
    }
}
