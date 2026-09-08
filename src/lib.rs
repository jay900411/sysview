//! sysview — Linux 系統觀測工具。
//!
//! 分層（UI 絕不直接讀 `/proc`）：
//!
//! ```text
//! collectors/   採樣、解析、差分、可用性判斷
//!      ↓
//! metrics/      正規化模型 + provenance + knowledge layer
//!      ↓
//! ui/           只負責畫
//! ```
//!
//! 另有 `privilege/`：非特權 TUI 透過 `sudo` 呼叫最小化的 `sysview-priv`
//! helper 取得管理員資訊。TUI 本身**永遠不以 root 執行**。

pub mod app;
pub mod banner;
pub mod collectors;
pub mod config;
pub mod error;
pub mod metrics;
pub mod privilege;
pub mod scoreboard;
pub mod snapshot;
pub mod theme;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub mod ui;
