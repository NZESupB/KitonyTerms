//! 发布正文只转换为受控节点，不注入 HTML 或加载远端图片。
use crate::i18n::texts;
use dioxus::prelude::*;
use kt_config::AppLanguage;
use pulldown_cmark::{Event, Options, Parser, Tag};

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Group,
    Paragraph,
    Heading,
    List(Option<u64>),
    Item,
    Strong,
    Emphasis,
    Strike,
    Quote,
    CodeBlock,
    Link(String),
}
#[derive(Clone, Debug, PartialEq)]
enum Node {
    Text(String),
    Code(String),
    Break,
    Rule,
    Container(Kind, Vec<Node>),
}

fn safe_link(destination: &str) -> Option<String> {
    let url = url::Url::parse(destination).ok()?;
    (matches!(url.scheme(), "https" | "http") && url.host_str().is_some()).then(|| url.to_string())
}

fn parse(body: &str) -> Vec<Node> {
    let mut stack = vec![(Kind::Group, Vec::new())];
    let mut flattened = 0usize;
    for event in Parser::new_ext(body, Options::ENABLE_STRIKETHROUGH) {
        match event {
            Event::Start(tag) => {
                // 有界嵌套避免恶意正文导致递归渲染栈溢出。
                if stack.len() >= 32 || flattened > 0 {
                    flattened += 1;
                    continue;
                }
                let kind = match tag {
                    Tag::Paragraph => Kind::Paragraph,
                    Tag::Heading { .. } => Kind::Heading,
                    Tag::List(start) => Kind::List(start),
                    Tag::Item => Kind::Item,
                    Tag::Strong => Kind::Strong,
                    Tag::Emphasis => Kind::Emphasis,
                    Tag::Strikethrough => Kind::Strike,
                    Tag::BlockQuote(_) => Kind::Quote,
                    Tag::CodeBlock(_) => Kind::CodeBlock,
                    Tag::Link { dest_url, .. } => {
                        safe_link(&dest_url).map(Kind::Link).unwrap_or(Kind::Group)
                    }
                    // 图片只显示替代文字；其他未启用结构保留其文本。
                    _ => Kind::Group,
                };
                stack.push((kind, Vec::new()));
            }
            Event::End(_) => {
                if flattened > 0 {
                    flattened -= 1;
                } else if stack.len() > 1 {
                    let (kind, children) = stack.pop().unwrap();
                    stack
                        .last_mut()
                        .unwrap()
                        .1
                        .push(Node::Container(kind, children));
                }
            }
            event => {
                let node = match event {
                    Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                        Node::Text(text.into_string())
                    }
                    Event::Code(text) => Node::Code(text.into_string()),
                    Event::SoftBreak => Node::Text("\n".into()),
                    Event::HardBreak => Node::Break,
                    Event::Rule => Node::Rule,
                    _ => continue,
                };
                stack.last_mut().unwrap().1.push(node);
            }
        }
    }
    stack.pop().unwrap().1
}

fn render(nodes: &[Node]) -> Element {
    rsx! {
        for node in nodes {
            {match node {
                Node::Text(text) => rsx! { "{text}" },
                Node::Code(text) => rsx! { code { "{text}" } },
                Node::Break => rsx! { br {} },
                Node::Rule => rsx! { hr {} },
                Node::Container(kind, children) => {
                    let content = render(children);
                    match kind {
                        Kind::Group => content,
                        Kind::Paragraph => rsx! { p { {content} } },
                        Kind::Heading => rsx! { h4 { {content} } },
                        Kind::List(Some(start)) => rsx! { ol { start: "{start}", {content} } },
                        Kind::List(None) => rsx! { ul { {content} } },
                        Kind::Item => rsx! { li { {content} } },
                        Kind::Strong => rsx! { strong { {content} } },
                        Kind::Emphasis => rsx! { em { {content} } },
                        Kind::Strike => rsx! { del { {content} } },
                        Kind::Quote => rsx! { blockquote { {content} } },
                        Kind::CodeBlock => rsx! { pre { code { {content} } } },
                        Kind::Link(href) => rsx! { a { href: href.clone(), target: "_blank", rel: "noopener noreferrer", {content} } },
                    }
                }
            }}
        }
    }
}

#[component]
pub(super) fn ReleaseNotes(body: String, truncated: bool, language: AppLanguage) -> Element {
    let mut expanded = use_signal(|| false);
    let nodes = use_memo(use_reactive((&body,), |(body,)| parse(&body)));
    let t = texts(language).app;
    let long = body.len() > 700 || body.lines().count() > 8;
    rsx! {
        if body.trim().is_empty() {
            p { class: "update-empty-notes", "{t.update_notes_empty}" }
        } else {
            div {
                id: "update-release-notes",
                class: if expanded() { "update-markdown is-expanded" } else { "update-markdown" },
                tabindex: "0",
                aria_label: t.update_notes,
                {render(&nodes.read())}
            }
            if long {
                button {
                    class: "update-text-button",
                    r#type: "button",
                    aria_expanded: expanded(),
                    aria_controls: "update-release-notes",
                    onclick: move |_| expanded.toggle(),
                    if expanded() { "{t.update_show_less}" } else { "{t.update_show_more}" }
                }
            }
        }
        if truncated {
            p { class: "update-notes-hint", "{t.update_notes_truncated}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markdown_keeps_html_as_text_and_filters_link_schemes() {
        let nodes = parse("## Changes\n\n- **Fast** and `safe`\n\n<script>alert(1)</script>\n\n[bad](javascript:alert) ![alt](https://example.com/image) [good](https://example.com/)");
        let debug = format!("{nodes:?}");
        assert!(debug.contains("Heading"));
        assert!(debug.contains("Strong"));
        assert!(debug.contains("<script>"));
        assert!(!debug.contains("Link(\"javascript:"));
        assert!(!debug.contains("example.com/image"));
        assert!(debug.contains("Link(\"https://example.com/\")"));
        for destination in [
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
            "//example.com",
            "https:",
        ] {
            assert!(safe_link(destination).is_none());
        }
    }
    #[test]
    fn deeply_nested_notes_are_bounded() {
        let nodes = parse(&format!("{}text", "> ".repeat(1000)));
        fn depth(nodes: &[Node]) -> usize {
            nodes
                .iter()
                .map(|node| match node {
                    Node::Container(_, children) => 1 + depth(children),
                    _ => 0,
                })
                .max()
                .unwrap_or(0)
        }
        assert!(depth(&nodes) <= 31);
        assert!(format!("{nodes:?}").contains("text"));
    }
}
