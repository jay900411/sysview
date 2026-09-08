//! 非特權 TUI 與特權 helper 之間的協定。
//!
//! # 安全設計
//!
//! 這個介面刻意做得**很窄**。helper 以 root 執行，所以必須假設呼叫端
//! （也就是 sysview TUI）**已經被攻陷**，介面本身仍要安全。
//!
//! 因此：
//!
//! * 沒有任何操作接受**路徑**。要掃某個使用者的家目錄，helper 自己去
//!   password database 查 —— 這樣路徑穿越（traversal）在架構上就不可能發生。
//! * 沒有任何操作接受**指令字串**。沒有 `exec`、沒有 `sh -c`、沒有 shell。
//! * 訊號只接受一個**極小的 allowlist**（TERM / INT / HUP），不接受任意數字。
//! * 會改變系統狀態的操作必須同時帶 `starttime`，helper 執行前會重新比對，
//!   避免 PID 重用造成的 confused deputy（你以為要殺 A，實際殺到剛好接手同一個
//!   PID 的 B）。
//! * 協定有版本號，helper 與 TUI 版本不合時直接拒絕，不做「盡力而為」的猜測。

use serde::{Deserialize, Serialize};

/// 協定版本。helper 與客戶端必須完全相同。
pub const PROTOCOL_VERSION: u32 = 1;

/// 使用者名稱的長度上限。遠比任何真實系統寬鬆，只是為了擋掉病態輸入。
const MAX_USERNAME: usize = 64;

/// helper 支援的全部操作。**這就是完整的 allowlist**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Operation {
    /// 探測：helper 在不在、版本對不對、我們是不是 root。唯讀且無副作用。
    Capability,
    /// 每個使用者的家目錄用量（唯讀，按需執行）。
    StorageUsers,
    /// 某個使用者家目錄的第一層明細。**只接受使用者名稱，不接受路徑。**
    StorageUserDetail { user: String },
    /// 每個使用者的記憶體用量彙總（含 PSS，如果讀得到）。
    UserMemory,
    /// 每個使用者的 GPU VRAM 用量彙總。
    GpuUsers,
    /// socket → PID → 使用者 對應。
    SocketMap,
    /// 單一行程的深入資訊（cgroup、affinity、排程器、fd 數量）。
    ProcessDetail { pid: i32 },
    /// 送出訊號。**唯一會改變系統狀態的操作之一。**
    ProcessSignal {
        pid: i32,
        starttime: u64,
        signal: SafeSignal,
    },
    /// 調整 nice 值。**會改變系統狀態。**
    Renice { pid: i32, starttime: u64, nice: i8 },
}

impl Operation {
    /// 這個操作會不會改變系統狀態。會的話 UI 必須二次確認、helper 必須寫 audit。
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::ProcessSignal { .. } | Self::Renice { .. })
    }

    /// 給 audit log 用的短名稱。
    pub fn name(&self) -> &'static str {
        match self {
            Self::Capability => "capability",
            Self::StorageUsers => "storage-users",
            Self::StorageUserDetail { .. } => "storage-user-detail",
            Self::UserMemory => "user-memory",
            Self::GpuUsers => "gpu-users",
            Self::SocketMap => "socket-map",
            Self::ProcessDetail { .. } => "process-detail",
            Self::ProcessSignal { .. } => "process-signal",
            Self::Renice { .. } => "renice",
        }
    }

    /// 轉成傳給 helper 的 argv。
    ///
    /// **回傳的是參數向量而不是命令列字串**，所以不存在 quoting 問題，
    /// 也永遠不會經過 shell。
    pub fn to_argv(&self) -> Vec<String> {
        let s = |x: &str| x.to_owned();
        match self {
            Self::Capability => vec![s("capability")],
            Self::StorageUsers => vec![s("storage-users")],
            Self::StorageUserDetail { user } => {
                vec![s("storage-user-detail"), s("--user"), user.clone()]
            }
            Self::UserMemory => vec![s("user-memory")],
            Self::GpuUsers => vec![s("gpu-users")],
            Self::SocketMap => vec![s("socket-map")],
            Self::ProcessDetail { pid } => {
                vec![s("process-detail"), s("--pid"), pid.to_string()]
            }
            Self::ProcessSignal {
                pid,
                starttime,
                signal,
            } => vec![
                s("process-signal"),
                s("--pid"),
                pid.to_string(),
                s("--starttime"),
                starttime.to_string(),
                s("--signal"),
                signal.name().to_owned(),
            ],
            Self::Renice {
                pid,
                starttime,
                nice,
            } => vec![
                s("renice"),
                s("--pid"),
                pid.to_string(),
                s("--starttime"),
                starttime.to_string(),
                s("--nice"),
                nice.to_string(),
            ],
        }
    }

    /// **helper 端**的參數驗證。
    ///
    /// 這是 defense in depth：客戶端已經驗過一次，但 helper 完全不信任客戶端，
    /// 所以在真的動手之前一定會再驗一次。
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::StorageUserDetail { user } => validate_username(user),
            Self::ProcessDetail { pid } => validate_pid(*pid),
            Self::ProcessSignal { pid, starttime, .. } => {
                validate_pid(*pid)?;
                validate_starttime(*starttime)
            }
            Self::Renice {
                pid,
                starttime,
                nice,
            } => {
                validate_pid(*pid)?;
                validate_starttime(*starttime)?;
                // 只允許調低優先度（提高 nice 值）。提高優先度是另一種
                // 資源濫用途徑，這版不開放。
                if *nice < 0 {
                    return Err(ValidationError::NiceOutOfRange);
                }
                if *nice > 19 {
                    return Err(ValidationError::NiceOutOfRange);
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// 允許送出的訊號。**刻意不接受任意訊號編號。**
///
/// 沒有 SIGKILL：那會讓行程沒有機會清理，對資料庫或寫入中的檔案很危險。
/// 使用者要 KILL 的話，請自己用 `kill -9` —— 那時責任明確在他自己身上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SafeSignal {
    Term,
    Int,
    Hup,
}

impl SafeSignal {
    pub fn name(self) -> &'static str {
        match self {
            Self::Term => "TERM",
            Self::Int => "INT",
            Self::Hup => "HUP",
        }
    }
    pub fn number(self) -> i32 {
        match self {
            Self::Term => libc::SIGTERM,
            Self::Int => libc::SIGINT,
            Self::Hup => libc::SIGHUP,
        }
    }
    /// 解析訊號名稱。**只認 allowlist 上的三個名字**，不接受數字。
    pub fn parse(s: &str) -> Result<Self, ValidationError> {
        match s.trim().to_ascii_uppercase().as_str() {
            "TERM" | "SIGTERM" => Ok(Self::Term),
            "INT" | "SIGINT" => Ok(Self::Int),
            "HUP" | "SIGHUP" => Ok(Self::Hup),
            _ => Err(ValidationError::SignalNotAllowed),
        }
    }
    pub fn all() -> &'static [SafeSignal] {
        &[Self::Term, Self::Int, Self::Hup]
    }
    /// 給 UI 顯示的說明，讓使用者知道自己在做什麼。
    pub fn description(self) -> &'static str {
        match self {
            Self::Term => "請求行程正常結束（可被攔截處理）",
            Self::Int => "等同於在終端機按 Ctrl-C",
            Self::Hup => "掛斷；多數 daemon 會用它重新載入設定",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("使用者名稱不合法")]
    BadUsername,
    #[error("PID 不合法")]
    BadPid,
    #[error("starttime 不合法")]
    BadStarttime,
    #[error("不允許的訊號（只接受 TERM / INT / HUP）")]
    SignalNotAllowed,
    #[error("nice 值必須在 0–19 之間（不開放提高優先度）")]
    NiceOutOfRange,
    #[error("未知的操作")]
    UnknownOperation,
    #[error("協定版本不符")]
    VersionMismatch,
}

/// 使用者名稱驗證。
///
/// 即使名稱最後只會被拿去查 password database（而不是組成路徑或指令），
/// 還是嚴格限制字元集 —— 這是 defense in depth，也擋掉 NUL 注入。
pub fn validate_username(u: &str) -> Result<(), ValidationError> {
    if u.is_empty() || u.len() > MAX_USERNAME {
        return Err(ValidationError::BadUsername);
    }
    // POSIX 可攜帳號名稱字元集，外加常見的 . 與 $（Samba 機器帳號）
    let ok = u
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'$'));
    if !ok {
        return Err(ValidationError::BadUsername);
    }
    // 開頭不允許 '-'，避免被誤解成命令列選項
    if u.starts_with('-') {
        return Err(ValidationError::BadUsername);
    }
    // 全部由點組成（"." / ".."）在檔案系統語意裡代表目錄，
    // 即使這裡只拿去查 passwd，也一律拒絕 —— defense in depth。
    if u.bytes().all(|c| c == b'.') {
        return Err(ValidationError::BadUsername);
    }
    Ok(())
}

pub fn validate_pid(pid: i32) -> Result<(), ValidationError> {
    // 1 是 init。0 與負數在 kill(2) 裡代表「整個 process group」或
    // 「所有行程」—— 那正是我們絕對要擋掉的東西。
    if pid <= 0 {
        return Err(ValidationError::BadPid);
    }
    Ok(())
}

fn validate_starttime(t: u64) -> Result<(), ValidationError> {
    // starttime 是開機以來的 jiffies。0 只有在解析失敗時才會出現。
    if t == 0 {
        return Err(ValidationError::BadStarttime);
    }
    Ok(())
}

/// helper 回傳的內容。永遠是 JSON。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub protocol_version: u32,
    pub operation: String,
    #[serde(flatten)]
    pub payload: Payload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Payload {
    Ok { data: serde_json::Value },
    Error { message: String },
}

impl Response {
    pub fn ok(op: &str, data: serde_json::Value) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            operation: op.to_owned(),
            payload: Payload::Ok { data },
        }
    }
    pub fn error(op: &str, msg: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            operation: op.to_owned(),
            payload: Payload::Error {
                message: msg.into(),
            },
        }
    }
    pub fn data(&self) -> Result<&serde_json::Value, &str> {
        match &self.payload {
            Payload::Ok { data } => Ok(data),
            Payload::Error { message } => Err(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_pid_zero_and_negative() {
        // kill(pid=0) 會殺掉整個 process group，kill(pid=-1) 會殺掉一切
        assert_eq!(validate_pid(0), Err(ValidationError::BadPid));
        assert_eq!(validate_pid(-1), Err(ValidationError::BadPid));
        assert_eq!(validate_pid(-9999), Err(ValidationError::BadPid));
        assert!(validate_pid(1).is_ok());
        assert!(validate_pid(12345).is_ok());
    }

    #[test]
    fn rejects_path_traversal_in_username() {
        for bad in [
            "../root",
            "..",
            "/etc/passwd",
            "a/b",
            "user\0name",
            "-rf",
            "",
        ] {
            assert!(validate_username(bad).is_err(), "{bad:?} 必須被拒絕");
        }
    }

    #[test]
    fn rejects_shell_metacharacters_in_username() {
        for bad in [
            "a;rm -rf /",
            "a|b",
            "a`id`",
            "a$(id)",
            "a&b",
            "a>b",
            "a b",
            "a'b",
            "a\"b",
        ] {
            assert!(
                validate_username(bad).is_err(),
                "{bad:?} 含 shell 元字元，必須拒絕"
            );
        }
    }

    #[test]
    fn rejects_overlong_username() {
        assert!(validate_username(&"a".repeat(65)).is_err());
        assert!(validate_username(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn accepts_realistic_usernames() {
        for ok in [
            "jay",
            "alice",
            "AB123_cd",
            "abc123456789",
            "svc.backup",
            "machine$",
        ] {
            assert!(
                validate_username(ok).is_ok(),
                "{ok:?} 是合法帳號名，不該被拒絕"
            );
        }
    }

    #[test]
    fn signal_allowlist_rejects_everything_else() {
        assert_eq!(SafeSignal::parse("TERM"), Ok(SafeSignal::Term));
        assert_eq!(SafeSignal::parse("sigint"), Ok(SafeSignal::Int));
        for bad in [
            "KILL",
            "SIGKILL",
            "9",
            "-9",
            "STOP",
            "SEGV",
            "",
            "TERM;KILL",
        ] {
            assert_eq!(
                SafeSignal::parse(bad),
                Err(ValidationError::SignalNotAllowed),
                "{bad:?} 不在 allowlist 上"
            );
        }
    }

    #[test]
    fn sigkill_is_deliberately_unavailable() {
        assert!(
            !SafeSignal::all()
                .iter()
                .any(|s| s.number() == libc::SIGKILL),
            "SIGKILL 不給行程清理的機會，刻意不提供"
        );
    }

    #[test]
    fn renice_only_allows_lowering_priority() {
        let mk = |n| Operation::Renice {
            pid: 100,
            starttime: 42,
            nice: n,
        };
        assert!(mk(0).validate().is_ok());
        assert!(mk(19).validate().is_ok());
        assert_eq!(
            mk(-1).validate(),
            Err(ValidationError::NiceOutOfRange),
            "不開放提高優先度"
        );
        assert_eq!(mk(20).validate(), Err(ValidationError::NiceOutOfRange));
    }

    #[test]
    fn mutations_require_starttime_to_prevent_pid_reuse() {
        let op = Operation::ProcessSignal {
            pid: 100,
            starttime: 0,
            signal: SafeSignal::Term,
        };
        assert_eq!(
            op.validate(),
            Err(ValidationError::BadStarttime),
            "沒有 starttime 就無法防止 PID 重用"
        );
    }

    #[test]
    fn identifies_mutations_correctly() {
        assert!(!Operation::Capability.is_mutation());
        assert!(!Operation::StorageUsers.is_mutation());
        assert!(!Operation::ProcessDetail { pid: 1 }.is_mutation());
        assert!(Operation::ProcessSignal {
            pid: 1,
            starttime: 1,
            signal: SafeSignal::Term
        }
        .is_mutation());
        assert!(Operation::Renice {
            pid: 1,
            starttime: 1,
            nice: 5
        }
        .is_mutation());
    }

    #[test]
    fn argv_never_contains_shell_syntax() {
        let ops = [
            Operation::Capability,
            Operation::StorageUsers,
            Operation::StorageUserDetail {
                user: "alice".into(),
            },
            Operation::UserMemory,
            Operation::GpuUsers,
            Operation::SocketMap,
            Operation::ProcessDetail { pid: 42 },
            Operation::ProcessSignal {
                pid: 42,
                starttime: 9,
                signal: SafeSignal::Term,
            },
            Operation::Renice {
                pid: 42,
                starttime: 9,
                nice: 10,
            },
        ];
        for op in ops {
            let argv = op.to_argv();
            assert!(!argv.is_empty());
            for a in &argv {
                assert!(
                    !a.contains(';')
                        && !a.contains('|')
                        && !a.contains('&')
                        && !a.contains('`')
                        && !a.contains('$')
                        && !a.contains('\n'),
                    "argv 元素 {a:?} 含 shell 元字元"
                );
            }
        }
    }

    #[test]
    fn no_operation_accepts_a_path() {
        // 這是架構層級的保證：只要協定裡沒有路徑欄位，
        // 路徑穿越就不可能發生。
        let json = serde_json::to_string(&Operation::StorageUserDetail {
            user: "alice".into(),
        })
        .unwrap();
        assert!(!json.contains('/'), "協定不該出現任何路徑");
    }

    #[test]
    fn response_roundtrips_through_json() {
        let r = Response::ok("storage-users", serde_json::json!({"users": []}));
        let s = serde_json::to_string(&r).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        assert_eq!(back.protocol_version, PROTOCOL_VERSION);
        assert!(back.data().is_ok());

        let e = Response::error("renice", "not permitted");
        let s = serde_json::to_string(&e).unwrap();
        let back: Response = serde_json::from_str(&s).unwrap();
        assert_eq!(back.data().unwrap_err(), "not permitted");
    }

    #[test]
    fn every_safe_signal_has_a_description() {
        for s in SafeSignal::all() {
            assert!(
                !s.description().is_empty(),
                "{} 缺少給使用者的說明",
                s.name()
            );
        }
    }
}
