//! 特權層：讓管理員取得更深入的資訊，但**絕不讓一般使用者越權**。
//!
//! # 架構
//!
//! ```text
//! ┌─────────────────────────┐
//! │ sysview                 │  一般使用者身分執行
//! │ 非特權 TUI              │  永遠不是 root
//! └────────────┬────────────┘
//!              │ argv（沒有 shell、沒有路徑、沒有指令字串）
//!              ▼
//!         ┌─────────┐
//!         │  sudo   │  ← sudoers / PAM / LDAP 決定准不准
//!         └────┬────┘
//!              ▼
//! ┌─────────────────────────┐
//! │ sysview-priv            │  root，但**不是 setuid**
//! │ 最小化 helper           │  只做 allowlist 上的事
//! └─────────────────────────┘
//! ```
//!
//! # 三條紅線
//!
//! 1. **整支 TUI 絕不以 root 執行。** `sudo sysview` 不是建議用法。
//! 2. **helper 絕不 setuid。** 提權只能經過 sudo，這樣授權才會經過 sudoers。
//! 3. **授權由 sudo 決定，不由我們決定。** 不看使用者在不在 sudo group。

pub mod client;
pub mod protocol;

pub use client::{PrivError, PrivilegeClient, PrivilegeState};
pub use protocol::{Operation, SafeSignal};

/// 整支程式是不是以 root 在跑。
///
/// 是的話 UI 會顯示警告：這不是建議的使用方式。
pub fn running_as_root() -> bool {
    crate::collectors::util::effective_uid() == 0
}

/// 使用者不該用 `sudo sysview`，要說清楚為什麼。
pub const ROOT_WARNING: &str =
    "以 root 執行整個 TUI 並非建議做法。sysview 設計為以一般使用者身分執行，\
     只有需要管理員資訊時才透過 sudo 呼叫最小化的 sysview-priv helper。";
