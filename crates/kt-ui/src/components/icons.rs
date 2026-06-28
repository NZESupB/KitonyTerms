//! KitonyTerms 线性图标组件。

use dioxus::prelude::*;

#[component]
pub fn Icon(name: &'static str) -> Element {
    rsx! {
        svg {
            class: "kt-icon kt-icon-{name}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            IconPath { name }
        }
    }
}

#[component]
fn IconPath(name: &'static str) -> Element {
    match name {
        "connect" => rsx! {
            path { d: "M5 12 11 6" }
            path { d: "M5 12 11 18" }
            path { d: "M13 18H21" }
        },
        "sessions" => rsx! {
            path { d: "M4 5H20V17H4z" }
            path { d: "M8 21H16" }
            path { d: "M10 17V21" }
            path { d: "M14 17V21" }
        },
        "folder" => rsx! {
            path { d: "M3 7H9L11 9H21V19H3z" }
        },
        "monitor" => rsx! {
            path { d: "M3 13H7L10 5L14 19L17 11H21" }
        },
        "settings" => rsx! {
            path { d: "M12 8A4 4 0 1 0 12 16A4 4 0 1 0 12 8" }
            path { d: "M12 2V5" }
            path { d: "M12 19V22" }
            path { d: "M4.9 4.9L7 7" }
            path { d: "M17 17L19.1 19.1" }
            path { d: "M2 12H5" }
            path { d: "M19 12H22" }
            path { d: "M4.9 19.1L7 17" }
            path { d: "M17 7L19.1 4.9" }
        },
        "search" => rsx! {
            path { d: "M10.5 4A6.5 6.5 0 1 0 10.5 17A6.5 6.5 0 1 0 10.5 4" }
            path { d: "M16 16L21 21" }
        },
        "bell" => rsx! {
            path { d: "M6 9A6 6 0 0 1 18 9C18 16 21 17 21 17H3S6 16 6 9" }
            path { d: "M10 21H14" }
        },
        "shield" => rsx! {
            path { d: "M12 3L20 6V12C20 17 16.5 20 12 21C7.5 20 4 17 4 12V6z" }
            path { d: "M9 12L11 14L15 10" }
        },
        "add" => rsx! {
            path { d: "M12 5V19" }
            path { d: "M5 12H19" }
        },
        "more" => rsx! {
            path { d: "M5 12H5.01" }
            path { d: "M12 12H12.01" }
            path { d: "M19 12H19.01" }
        },
        "filter" => rsx! {
            path { d: "M4 5H20L14 12V19L10 21V12z" }
        },
        "chevron-down" => rsx! {
            path { d: "M6 9L12 15L18 9" }
        },
        "close" => rsx! {
            path { d: "M6 6L18 18" }
            path { d: "M18 6L6 18" }
        },
        "edit" => rsx! {
            path { d: "M4 20H8L19 9L15 5L4 16z" }
            path { d: "M13 7L17 11" }
        },
        "trash" => rsx! {
            path { d: "M4 7H20" }
            path { d: "M9 7V5H15V7" }
            path { d: "M7 7L8 21H16L17 7" }
            path { d: "M10 11V17" }
            path { d: "M14 11V17" }
        },
        "split" => rsx! {
            path { d: "M4 4H20V20H4z" }
            path { d: "M12 4V20" }
            path { d: "M4 12H20" }
        },
        "split-horizontal" => rsx! {
            path { d: "M4 5H20V19H4z" }
            path { d: "M4 12H20" }
        },
        "split-vertical" => rsx! {
            path { d: "M4 5H20V19H4z" }
            path { d: "M12 5V19" }
        },
        "clear" => rsx! {
            path { d: "M6 6H18V18H6z" }
            path { d: "M9 9L15 15" }
            path { d: "M15 9L9 15" }
        },
        "back" => rsx! {
            path { d: "M15 6L9 12L15 18" }
        },
        "refresh" => rsx! {
            path { d: "M20 12A8 8 0 0 1 6.3 17.7" }
            path { d: "M4 12A8 8 0 0 1 17.7 6.3" }
            path { d: "M17 3V7H21" }
            path { d: "M7 21V17H3" }
        },
        "upload" => rsx! {
            path { d: "M12 19V5" }
            path { d: "M6 11L12 5L18 11" }
            path { d: "M5 21H19" }
        },
        "download" => rsx! {
            path { d: "M12 5V19" }
            path { d: "M6 13L12 19L18 13" }
            path { d: "M5 21H19" }
        },
        "list" => rsx! {
            path { d: "M8 6H21" }
            path { d: "M8 12H21" }
            path { d: "M8 18H21" }
            path { d: "M3 6H3.01" }
            path { d: "M3 12H3.01" }
            path { d: "M3 18H3.01" }
        },
        "file" => rsx! {
            path { d: "M6 3H14L19 8V21H6z" }
            path { d: "M14 3V8H19" }
        },
        "cpu" => rsx! {
            path { d: "M8 8H16V16H8z" }
            path { d: "M4 10H8" }
            path { d: "M4 14H8" }
            path { d: "M16 10H20" }
            path { d: "M16 14H20" }
            path { d: "M10 4V8" }
            path { d: "M14 4V8" }
            path { d: "M10 16V20" }
            path { d: "M14 16V20" }
        },
        "memory" => rsx! {
            path { d: "M5 8H19V16H5z" }
            path { d: "M8 8V16" }
            path { d: "M12 8V16" }
            path { d: "M16 8V16" }
            path { d: "M7 18V20" }
            path { d: "M12 18V20" }
            path { d: "M17 18V20" }
        },
        "load" => rsx! {
            path { d: "M12 20A8 8 0 1 0 12 4A8 8 0 1 0 12 20" }
            path { d: "M12 12L16 8" }
            path { d: "M7 14H9" }
            path { d: "M15 14H17" }
        },
        "network" => rsx! {
            path { d: "M12 4V9" }
            path { d: "M6 20V15" }
            path { d: "M18 20V15" }
            path { d: "M12 9L6 15" }
            path { d: "M12 9L18 15" }
            path { d: "M6 15L18 15" }
        },
        _ => rsx! {
            path { d: "M4 4H20V20H4z" }
        },
    }
}

#[component]
pub fn AppLogo(size: &'static str) -> Element {
    rsx! {
        span {
            class: "app-logo app-logo-{size}",
            aria_hidden: "true",
            span { class: "app-logo-chevron" }
            span { class: "app-logo-cursor" }
        }
    }
}
