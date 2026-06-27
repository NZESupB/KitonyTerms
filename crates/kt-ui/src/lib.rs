//! KitonyTerms UI 组件库
//!
//! 提供所有 UI 组件，包括终端渲染、会话列表、SFTP 面板、资源监控等。

pub mod components;
pub mod i18n;
pub mod state;
pub mod store;

// 重新导出核心组件
pub use components::app::App;
pub use state::{AppState, GlobalState, SessionState};
pub use store::Store;
