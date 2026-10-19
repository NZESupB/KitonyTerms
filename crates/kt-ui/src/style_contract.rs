//! 界面文字尺寸的样式不变量。
//!
//! 界面字体族与界面字号通过 `app.css` 的 `--kt-ui-font` / `--kt-ui-scale` 与
//! `--kt-text-*` 令牌生效：新增界面文字如果仍写死 px，就不会随界面字号缩放。
//! 这里把「除终端区与 iOS 输入保护外不存在 px 字号」固化成测试，避免后续回归。

#[cfg(test)]
mod tests {
    /// 终端区：字号由终端设置内联决定，行高按 `em` 与该字号绑定，必须保持 px。
    const TERMINAL_SELECTORS: &[&str] = &[
        ".terminal-surface",
        ".terminal-gutter",
        ".terminal-placeholder",
    ];

    /// 令牌根：`--kt-ui-scale` / `--kt-ui-font` 内联写在这两个根上，
    /// `:root` 保持同一份默认值，因此三处都要声明令牌。
    const TOKEN_ROOTS: &[&str] = &[":root,", ".kt-window,", ".software-update-layer {"];

    fn css() -> &'static str {
        include_str!("assets/app.css")
    }

    /// 返回 offset 处声明的规则选择器（CSS 规则不嵌套，@media 只包裹规则）。
    fn selector_for(content: &str, offset: usize) -> String {
        let brace = content[..offset].rfind('{').expect("声明必须在规则内");
        let start = content[..brace]
            .rfind(['}', '{'])
            .map(|index| index + 1)
            .unwrap_or(0);
        let mut selector = &content[start..brace];
        if let Some(index) = selector.rfind('{') {
            selector = &selector[index + 1..];
        }
        selector.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn interface_font_sizes_use_scalable_tokens() {
        let content = css();
        let mut offenders = Vec::new();
        let mut cursor = 0;
        while let Some(index) = content[cursor..].find("font-size:") {
            let start = cursor + index;
            let end = content[start..]
                .find([';', '}'])
                .map(|offset| start + offset)
                .expect("声明必须以 ; 或 } 结束");
            let declaration = content[start..end].to_string();
            let selector = selector_for(content, start);
            cursor = end;
            if declaration.contains("var(--kt-text-")
                || declaration.contains("inherit")
                || declaration.contains("max(16px, var(--kt-text-16))")
            {
                continue;
            }
            if TERMINAL_SELECTORS
                .iter()
                .any(|terminal| selector.contains(terminal))
            {
                continue;
            }
            offenders.push(format!("{selector} -> {declaration}"));
        }
        assert!(
            offenders.is_empty(),
            "界面文字尺寸必须使用 --kt-text-* 令牌（终端区与 iOS 16px 保护除外）：{offenders:?}"
        );
    }

    #[test]
    fn text_tokens_are_declared_for_every_token_root() {
        let content = css();
        for root in TOKEN_ROOTS {
            let start = content.find(root).expect("缺少令牌根声明");
            let end = content[start..]
                .find('}')
                .map(|offset| start + offset)
                .expect("令牌根规则必须闭合");
            let block = &content[start..end];
            for size in 10..=22 {
                let token = format!("--kt-text-{size}:");
                assert!(
                    block.contains(&token),
                    "{root} 缺少 {token}（界面文字令牌必须让每个根都能看到内联缩放值）"
                );
            }
        }
    }

    #[test]
    fn ui_font_variable_is_applied_to_both_render_roots() {
        let content = css();
        // 两个渲染根各自声明字体族：font-family 是继承的计算值，只在祖先声明无效。
        let expected = "font-family: var(--kt-ui-font, var(--kt-ui-font-default));";
        for selector in [
            ".kt-window {\n",
            ".software-update-layer { --update-ease-out",
        ] {
            let start = content.find(selector).expect("缺少渲染根规则块");
            let end = content[start..]
                .find('}')
                .map(|offset| start + offset)
                .expect("渲染根规则块必须闭合");
            assert!(
                content[start..end].contains(expected),
                "{selector} 必须声明 {expected}"
            );
        }
    }
}
