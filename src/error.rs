//! 全專案共用的錯誤型別。
//!
//! 設計重點：`/proc`、`/sys`、GPU、行程、掛載點、網路介面全都是**隨時會消失**的東西。
//! 行程可能在 `readdir` 與 `open` 之間結束，這是正常情況而不是 bug。
//! 因此本專案不用 `unwrap()`/`expect()` 處理執行期系統資訊，一律轉成
//! [`Unavailable`] 讓 UI 顯示 `n/a` / `permission denied`，而不是 panic。

use std::fmt;
use std::path::{Path, PathBuf};

/// 某個數值為什麼拿不到。UI 會照這個顯示不同文字與符號。
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Unavailable {
    /// 這個平台/裝置根本沒有這個介面（例如 i915 沒有 `gpu_busy_percent`）。
    Unsupported,
    /// 需要更高權限才讀得到（例如別人的 `/proc/<pid>/io`）。
    PermissionDenied,
    /// 讀取當下標的已經不存在（行程結束、裝置拔除、檔案系統卸載）。
    Vanished,
    /// 檔案在但內容不符預期。
    Malformed(String),
    /// 後端還沒回應（例如 NVML 尚未初始化完成）。
    Pending,
    /// 其他 I/O 錯誤。
    Io(String),
}

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "unsupported"),
            Self::PermissionDenied => write!(f, "permission denied"),
            Self::Vanished => write!(f, "vanished"),
            Self::Malformed(w) => write!(f, "malformed: {w}"),
            Self::Pending => write!(f, "pending"),
            Self::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl Unavailable {
    /// 把 `std::io::Error` 分類。ENOENT/ESRCH 視為「消失了」而不是錯誤。
    pub fn from_io(e: &std::io::Error) -> Self {
        use std::io::ErrorKind::*;
        match e.kind() {
            PermissionDenied => Self::PermissionDenied,
            NotFound => Self::Vanished,
            _ => match e.raw_os_error() {
                Some(libc::ESRCH) | Some(libc::ENODEV) | Some(libc::ENXIO) => Self::Vanished,
                Some(libc::ENOTSUP) | Some(libc::ENOSYS) => Self::Unsupported,
                _ => Self::Io(e.to_string()),
            },
        }
    }

    /// UI 上顯示的短字串。
    pub fn short(&self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::PermissionDenied => "permission required",
            Self::Vanished => "gone",
            Self::Malformed(_) => "bad data",
            Self::Pending => "…",
            Self::Io(_) => "n/a",
        }
    }
}

/// 讀某個路徑失敗時，同時保留路徑，方便診斷與 Explain 顯示來源。
#[derive(Debug, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct SourceError {
    pub path: PathBuf,
    pub reason: Unavailable,
}

impl SourceError {
    pub fn new(path: impl AsRef<Path>, reason: Unavailable) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            reason,
        }
    }
}

pub type Result<T> = std::result::Result<T, anyhow::Error>;
