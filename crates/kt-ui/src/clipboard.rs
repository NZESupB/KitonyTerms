//! 系统剪贴板读取。
//!
//! 桌面端直接读原生剪贴板：WKWebView / WebView2 / WebKitGTK 对
//! `navigator.clipboard.readText()` 都要求系统级粘贴确认（macOS 表现为点完
//! 「粘贴」菜单后还要再点一次系统弹出的 Paste 按钮），原生读取可完全绕开确认。
//! 移动端没有原生实现，仍由调用方回退到 WebView 的异步剪贴板 API。

/// 读取剪贴板文本；剪贴板为空或不含文本时返回 `Ok(None)`。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub fn read_text() -> Result<Option<String>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard_text(clipboard.get_text())
}

/// 把剪贴板读取结果归一化：空剪贴板不是错误，其余错误保留原始描述。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn clipboard_text(result: Result<String, arboard::Error>) -> Result<Option<String>, String> {
    match result {
        Ok(text) => Ok(Some(text)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(all(
    test,
    any(target_os = "windows", target_os = "linux", target_os = "macos")
))]
mod tests {
    use super::*;

    #[test]
    fn empty_clipboard_is_not_an_error() {
        assert_eq!(
            clipboard_text(Err(arboard::Error::ContentNotAvailable)),
            Ok(None)
        );
    }

    #[test]
    fn clipboard_text_is_returned_as_is() {
        assert_eq!(
            clipboard_text(Ok("ls -al\n".to_string())),
            Ok(Some("ls -al\n".to_string()))
        );
    }

    #[test]
    fn other_clipboard_failures_keep_their_description() {
        let error = clipboard_text(Err(arboard::Error::Unknown {
            description: "剪贴板被占用".to_string(),
        }))
        .unwrap_err();

        assert!(error.contains("剪贴板被占用"), "{error}");
    }
}
