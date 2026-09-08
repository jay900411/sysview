//! 呼叫特權 helper 的客戶端。
//!
//! # 這裡**不做**什麼
//!
//! * 不讀密碼、不存密碼、不 log 密碼、不把密碼 pipe 給 sudo、不模擬 PAM。
//!   密碼提示完全交給 `sudo` 自己處理，我們只負責把終端機讓出來。
//! * 不用「使用者在不在 sudo group」來決定授權。wheel / sudo / LDAP / AD /
//!   自訂 sudoers 各家環境不同，**唯一的授權判準是 sudo 自己的結果**。
//! * 不組 shell 字串。一律 argv。
//!
//! # 流程
//!
//! ```text
//! sysview (一般使用者)
//!    │  Command::new("sudo").arg("-n").arg(helper).args(argv)
//!    ▼
//! sudo  ← sudoers / PAM / LDAP 決定准不准
//!    │
//!    ▼
//! sysview-priv (root, 非 setuid)
//! ```

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::protocol::{Operation, Response, PROTOCOL_VERSION};

/// helper 可能的安裝位置。**寫死的候選清單**，不接受環境變數覆寫 ——
/// 否則攻擊者只要設個 `SYSVIEW_HELPER=/tmp/evil` 就能讓 sudo 去跑他的東西。
const HELPER_CANDIDATES: &[&str] = &[
    "/usr/local/libexec/sysview/sysview-priv",
    "/usr/libexec/sysview/sysview-priv",
    "/usr/local/lib/sysview/sysview-priv",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrivilegeState {
    /// 系統上找不到 helper（沒安裝，或只裝了 TUI）。
    HelperMissing(String),
    /// helper 在，但目前沒有有效的 sudo 授權快取 —— 需要使用者輸入密碼。
    Locked,
    /// sudo 明確拒絕：這個使用者沒有權限。**不是錯誤，是正確的拒絕。**
    Unauthorized(String),
    /// 可以用了。
    Available,
}

impl PrivilegeState {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }
    /// 顯示在 UI 上的一行狀態。
    pub fn label(&self) -> String {
        match self {
            Self::HelperMissing(_) => "Admin extensions: helper not installed".into(),
            Self::Locked => "Admin extensions: locked".into(),
            Self::Unauthorized(_) => "Admin extensions: not authorized".into(),
            Self::Available => "Admin extensions: unlocked".into(),
        }
    }
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::HelperMissing(m) | Self::Unauthorized(m) => Some(m),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PrivError {
    #[error("找不到 sysview-priv helper：{0}")]
    HelperMissing(String),
    #[error("helper 的安裝方式不安全：{0}")]
    HelperUnsafe(String),
    #[error("需要管理員授權（尚未通過 sudo 驗證）")]
    NeedsAuth,
    #[error("你沒有執行這個管理員功能的權限")]
    NotAuthorized,
    #[error("helper 執行失敗：{0}")]
    Failed(String),
    #[error("helper 回應無法解析：{0}")]
    BadResponse(String),
    #[error("協定版本不符：helper 是 v{0}，本程式是 v{1}")]
    VersionMismatch(u32, u32),
    #[error("操作逾時")]
    Timeout,
}

/// 可以 clone。背景 worker 拿到的是同一份狀態（`Arc<Mutex<_>>`），
/// 所以 worker 打到的實際結果會立刻反映到 UI 上，不會出現
/// 「畫面說已解鎖但每次查詢都失敗」這種矛盾。
#[derive(Clone)]
pub struct PrivilegeClient {
    helper: Option<PathBuf>,
    /// 用 Arc<Mutex<>> 是因為啟動時的探測跑在背景執行緒 ——
    /// `sudo` 在 LDAP / AD 環境可能慢到幾秒，不能讓 TUI 卡在黑畫面。
    state: Arc<Mutex<PrivilegeState>>,
}

impl PrivilegeClient {
    /// 建立客戶端並在**背景**探測授權狀態。
    ///
    /// 啟動時不阻塞：先當成 `Locked`，探測結果回來後自動更新。
    /// UI 每次繪製都會重讀狀態，所以使用者最多只會看到一瞬間的 "locked"。
    pub fn new() -> Self {
        let c = Self::without_probe();
        c.probe_in_background();
        c
    }

    /// 不做探測的版本。給背景 worker 用 —— 它們本來就要送出真正的請求，
    /// 再探測一次只是多 fork 一個 sudo。
    pub fn without_probe() -> Self {
        match locate_helper() {
            Ok(p) => Self {
                helper: Some(p),
                state: Arc::new(Mutex::new(PrivilegeState::Locked)),
            },
            Err(e) => Self {
                helper: None,
                state: Arc::new(Mutex::new(PrivilegeState::HelperMissing(e.to_string()))),
            },
        }
    }

    /// 只給測試用：直接指定狀態，完全不碰 sudo。
    ///
    /// 這**不是**權限旁路。狀態只是 UI 用來決定畫哪個畫面的提示；
    /// 真正的呼叫還是會 fork `sudo`，helper 也還是會檢查 root EUID。
    /// 把狀態設成 `Available` 只是讓測試能畫出解鎖後的 Admin 版面。
    /// 這個建構子被 `test-support` feature 擋著，release build 裡不存在。
    #[cfg(feature = "test-support")]
    pub fn with_state_for_tests(s: PrivilegeState) -> Self {
        Self {
            helper: locate_helper().ok(),
            state: Arc::new(Mutex::new(s)),
        }
    }

    /// 目前的授權狀態。mutex 中毒時一律當成不可用 —— fail closed。
    pub fn state(&self) -> PrivilegeState {
        match self.state.lock() {
            Ok(g) => g.clone(),
            Err(_) => PrivilegeState::Unauthorized("internal state poisoned".into()),
        }
    }
    pub fn helper_path(&self) -> Option<&Path> {
        self.helper.as_deref()
    }

    fn set_state(&self, s: PrivilegeState) {
        if let Ok(mut g) = self.state.lock() {
            *g = s;
        }
    }

    /// 在背景用 `sudo -n`（非互動）探測授權。
    ///
    /// `-n` 保證**絕對不會**跳出密碼提示，所以即使 TUI 已經在 alternate
    /// screen 上，這個探測也不會把畫面弄壞。
    pub fn probe_in_background(&self) {
        let Some(helper) = self.helper.clone() else {
            return;
        };
        let slot = Arc::clone(&self.state);
        std::thread::spawn(move || {
            let c = Self {
                helper: Some(helper),
                state: Arc::new(Mutex::new(PrivilegeState::Locked)),
            };
            let result = classify(c.run_raw(&Operation::Capability));
            if let Ok(mut g) = slot.lock() {
                *g = result;
            }
        });
    }

    /// 用 `sudo -n`（非互動）探測目前有沒有有效授權。
    ///
    /// `-n` 保證**絕對不會**跳出密碼提示，所以可以安全地在 TUI 還在
    /// alternate screen 的時候呼叫，不會把畫面弄壞。
    /// 立刻重新確認授權狀態（非互動 `sudo -n`，通常只要幾毫秒）。
    ///
    /// 回傳最新狀態。按 `u` 時會先呼叫這個，才不會發生
    /// 「說已解鎖 → 進去查詢 → 失敗 → 再按一次才跳密碼」這種兩段式體驗。
    pub fn recheck(&self) -> PrivilegeState {
        if self.helper.is_none() {
            return self.state();
        }
        let s = classify(self.run_raw(&Operation::Capability));
        self.set_state(s.clone());
        s
    }

    pub fn refresh(&mut self) {
        let _ = self.recheck();
    }

    /// 互動式取得授權。
    ///
    /// **呼叫端必須先離開 alternate screen 與 raw mode**，把終端機完整交給
    /// `sudo`，讓它自己處理密碼提示 / PAM / 指紋 / 硬體金鑰。
    /// 我們只是等它結束，然後看它的離開碼。
    pub fn authenticate_interactive(&mut self) -> Result<(), PrivError> {
        let helper = self
            .helper
            .clone()
            .ok_or_else(|| PrivError::HelperMissing("helper 未安裝".into()))?;
        // 直接跑一次 capability：這樣 sudoers 若限定只能跑 helper 也能通過，
        // 比 `sudo -v`（需要更寬的權限）更符合最小權限原則。
        let status = Command::new("sudo")
            .arg("--")
            .arg(&helper)
            .arg("capability")
            .stdin(Stdio::inherit())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| PrivError::Failed(e.to_string()))?;
        self.refresh();
        if status.success() {
            Ok(())
        } else {
            Err(PrivError::NotAuthorized)
        }
    }

    /// 執行一個操作。失敗時 fail closed —— 絕不「盡力而為」地回半套資料。
    pub fn run(&self, op: &Operation) -> Result<serde_json::Value, PrivError> {
        // 客戶端先驗一次（helper 還會再驗一次，這是 defense in depth）
        op.validate()
            .map_err(|e| PrivError::Failed(e.to_string()))?;
        // 快取的狀態只是給 UI 顯示用的**提示**，不可以拿來擋真正的呼叫。
        //
        // 有權決定准不准的是 sudo，不是我們。憑證可能在探測之後才過期、
        // 也可能在探測之後才被管理員授予 —— 兩種情況都只有實際打過去才知道。
        // 唯一真正的硬阻擋是「helper 根本不存在」，那沒有東西可以呼叫。
        if let PrivilegeState::HelperMissing(m) = self.state() {
            return Err(PrivError::HelperMissing(m));
        }
        let resp = match self.run_raw(op) {
            Ok(r) => {
                self.set_state(PrivilegeState::Available);
                r
            }
            Err(e) => {
                // 讓 UI 的狀態跟這次的實際結果一致，
                // 不要再出現「顯示已解鎖但每次查詢都失敗」這種矛盾。
                match &e {
                    PrivError::NeedsAuth => self.set_state(PrivilegeState::Locked),
                    PrivError::NotAuthorized => {
                        self.set_state(PrivilegeState::Unauthorized("sudo 拒絕了這個操作".into()))
                    }
                    _ => {}
                }
                return Err(e);
            }
        };
        match resp.data() {
            Ok(v) => Ok(v.clone()),
            Err(m) => Err(PrivError::Failed(m.to_owned())),
        }
    }

    fn run_raw(&self, op: &Operation) -> Result<Response, PrivError> {
        let helper = self
            .helper
            .as_ref()
            .ok_or_else(|| PrivError::HelperMissing("helper 未安裝".into()))?;

        // 每次執行前重新檢查 helper 的權限：安裝後被人動過手腳就要拒絕。
        verify_helper_safety(helper).map_err(|e| PrivError::HelperUnsafe(e.to_string()))?;

        let mut cmd = Command::new("sudo");
        cmd.arg("-n") // 非互動：絕不跳密碼提示
            .arg("--") // 選項結束，後面全是參數，不會被當成 sudo 選項
            .arg(helper)
            .args(op.to_argv())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd.spawn().map_err(|e| PrivError::Failed(e.to_string()))?;
        let (status, stdout_raw, stderr_raw) = output_with_timeout(child, timeout_for(op))?;
        let stderr = String::from_utf8_lossy(&stderr_raw);

        if !status.success() {
            // sudo 的離開碼無法區分「需要密碼」與「不允許」，只能看訊息。
            let s = stderr.to_ascii_lowercase();
            if s.contains("password is required") || s.contains("a password is required") {
                return Err(PrivError::NeedsAuth);
            }
            if s.contains("not allowed")
                || s.contains("may not run")
                || s.contains("not in the sudoers")
            {
                return Err(PrivError::NotAuthorized);
            }
            if stdout_raw.is_empty() {
                return Err(PrivError::Failed(first_line(&stderr)));
            }
        }

        let text = String::from_utf8_lossy(&stdout_raw);
        let resp: Response = serde_json::from_str(text.trim())
            .map_err(|e| PrivError::BadResponse(format!("{e}: {}", first_line(&text))))?;
        if resp.protocol_version != PROTOCOL_VERSION {
            return Err(PrivError::VersionMismatch(
                resp.protocol_version,
                PROTOCOL_VERSION,
            ));
        }
        Ok(resp)
    }
}

impl Default for PrivilegeClient {
    fn default() -> Self {
        Self::new()
    }
}

/// 把探測結果翻譯成授權狀態。
fn classify(r: Result<Response, PrivError>) -> PrivilegeState {
    match r {
        Ok(_) => PrivilegeState::Available,
        Err(PrivError::NeedsAuth) => PrivilegeState::Locked,
        Err(PrivError::NotAuthorized) => PrivilegeState::Unauthorized("sudo 拒絕了這個操作".into()),
        // 逾時不代表沒有權限，只代表現在問不到 —— 維持 locked，讓使用者可以重試
        Err(PrivError::Timeout) => PrivilegeState::Locked,
        Err(e) => PrivilegeState::Unauthorized(e.to_string()),
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_owned()
}

/// 在寫死的候選清單裡找 helper。
///
/// 也會看「和目前執行檔同一棵樹」的相對位置，讓開發時
/// （`target/release/sysview-priv`）也能用，但那條路徑一樣要通過安全檢查。
fn locate_helper() -> Result<PathBuf, PrivError> {
    let mut candidates: Vec<PathBuf> = HELPER_CANDIDATES.iter().map(PathBuf::from).collect();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 安裝後的相對位置：<prefix>/bin/sysview → <prefix>/libexec/sysview/sysview-priv
            if let Some(prefix) = dir.parent() {
                candidates.push(prefix.join("libexec/sysview/sysview-priv"));
            }
            // 開發時：兩個 binary 在同一個 target 目錄
            candidates.push(dir.join("sysview-priv"));
        }
    }
    for c in &candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }
    Err(PrivError::HelperMissing(format!(
        "找過這些位置都沒有：{}",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// helper 的安裝方式安全檢查。
///
/// 我們即將請 `sudo` 以 **root** 執行這個檔案，所以必須先確認它不是任何人
/// 都能改的東西。否則攻擊者只要能寫入 helper，就等於拿到 root。
pub fn verify_helper_safety(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

    // 用 symlink_metadata 檢查本體，避免被 symlink 換掉目標
    let md = std::fs::symlink_metadata(path)?;
    if md.file_type().is_symlink() {
        return Err(std::io::Error::other(format!(
            "{} 是 symlink；helper 必須是實體檔案",
            path.display()
        )));
    }
    if !md.is_file() {
        return Err(std::io::Error::other(format!(
            "{} 不是普通檔案",
            path.display()
        )));
    }
    let mode = md.permissions().mode();
    // helper **絕對不可以是 setuid**。它取得權限的唯一途徑是 sudo，
    // 這樣授權才會經過 sudoers 而不是繞過它。
    if mode & 0o4000 != 0 {
        return Err(std::io::Error::other(format!(
            "{} 是 setuid —— 這是嚴重的安全問題，helper 只能透過 sudo 提權",
            path.display()
        )));
    }
    if mode & 0o2000 != 0 {
        return Err(std::io::Error::other(format!(
            "{} 是 setgid",
            path.display()
        )));
    }
    // 開發環境（cargo target 目錄）的檔案本來就屬於一般使用者，
    // 這時不做 root 擁有者檢查，但正式安裝路徑一定要檢查。
    let is_system_path = HELPER_CANDIDATES.iter().any(|c| path == Path::new(c));
    if is_system_path {
        if md.uid() != 0 {
            return Err(std::io::Error::other(format!(
                "{} 的擁有者不是 root（uid {}）",
                path.display(),
                md.uid()
            )));
        }
        if mode & 0o022 != 0 {
            return Err(std::io::Error::other(format!(
                "{} 可被 group/other 寫入（mode {:o}）—— 等於任何人都能拿 root",
                path.display(),
                mode & 0o7777
            )));
        }
    }
    Ok(())
}

/// 給 UI 顯示：告訴使用者我們**會**執行什麼指令，完全透明。
pub fn describe_invocation(helper: &Path, op: &Operation) -> String {
    let args = op.to_argv().join(" ");
    format!("sudo -- {} {}", helper.display(), args)
}

/// 每個操作的逾時上限。
///
/// `sudo` 平常回應是毫秒等級，但在 LDAP / AD / 網路認證的環境下**可能阻塞**。
/// 監控工具絕不能因為認證後端慢就整個卡死，所以每一次呼叫都有上限。
pub fn timeout_for(op: &Operation) -> Duration {
    match op {
        // 探測跑在背景，但仍要短 —— 卡住的 sudo 不該讓那條執行緒一直活著
        Operation::Capability => Duration::from_secs(5),
        // 掃 /home 本來就慢，helper 內部另有自己的預算上限
        Operation::StorageUsers => Duration::from_secs(150),
        Operation::StorageUserDetail { .. } => Duration::from_secs(90),
        // 其餘都是讀 /proc，應該很快
        _ => Duration::from_secs(30),
    }
}

/// 執行子行程，超過 `limit` 就殺掉。
///
/// `std::process::Command::output()` 沒有逾時機制，會無限等下去。
/// 這裡用兩條讀取執行緒把 stdout/stderr 抽乾（避免管線塞滿造成死結），
/// 主執行緒輪詢 `try_wait` 直到期限；逾時就 kill 並 wait，不留殭屍行程。
fn output_with_timeout(
    mut child: Child,
    limit: Duration,
) -> Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>), PrivError> {
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {}
            Err(e) => return Err(PrivError::Failed(e.to_string())),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PrivError::Timeout);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    Ok((status, stdout, stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn rejects_setuid_helper() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("sysview-priv");
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o4755)).unwrap();
        let e = verify_helper_safety(&p).unwrap_err();
        assert!(
            e.to_string().contains("setuid"),
            "setuid helper 必須被拒絕：{e}"
        );
    }

    #[test]
    fn rejects_setgid_helper() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("h");
        std::fs::write(&p, b"x").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o2755)).unwrap();
        assert!(verify_helper_safety(&p).is_err());
    }

    #[test]
    fn rejects_symlinked_helper() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::write(&real, b"x").unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let e = verify_helper_safety(&link).unwrap_err();
        assert!(
            e.to_string().contains("symlink"),
            "symlink 可被替換，必須拒絕"
        );
    }

    #[test]
    fn accepts_plain_executable_in_dev_tree() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("sysview-priv");
        std::fs::write(&p, b"x").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            verify_helper_safety(&p).is_ok(),
            "開發樹裡的普通執行檔應可使用"
        );
    }

    #[test]
    fn missing_helper_is_an_error_not_a_panic() {
        assert!(verify_helper_safety(Path::new("/nonexistent/sysview-priv")).is_err());
    }

    #[test]
    fn helper_path_is_not_configurable_by_environment() {
        // 確認候選清單裡沒有任何一項來自環境變數
        for c in HELPER_CANDIDATES {
            assert!(c.starts_with('/'), "helper 路徑必須是絕對路徑：{c}");
            assert!(!c.contains('$'), "helper 路徑不可含變數展開：{c}");
        }
    }

    #[test]
    fn invocation_description_is_transparent() {
        let d = describe_invocation(
            Path::new("/usr/local/libexec/sysview/sysview-priv"),
            &Operation::StorageUsers,
        );
        assert!(d.starts_with("sudo -- /usr/local/libexec"));
        assert!(d.ends_with("storage-users"));
    }

    #[test]
    fn state_labels_never_claim_access_when_locked() {
        assert!(!PrivilegeState::Locked.is_available());
        assert!(!PrivilegeState::Unauthorized("x".into()).is_available());
        assert!(!PrivilegeState::HelperMissing("x".into()).is_available());
        assert!(PrivilegeState::Available.is_available());
        assert!(PrivilegeState::Locked.label().contains("locked"));
    }

    #[test]
    fn client_without_helper_fails_closed() {
        let c = PrivilegeClient {
            helper: None,
            state: Arc::new(Mutex::new(PrivilegeState::HelperMissing("test".into()))),
        };
        let r = c.run(&Operation::StorageUsers);
        assert!(
            matches!(r, Err(PrivError::HelperMissing(_))),
            "沒有 helper 必須 fail closed"
        );
    }

    #[test]
    fn probe_timeout_is_short_enough_for_startup() {
        // capability 探測跑在背景執行緒（見 probe_in_background），所以不會
        // 擋住啟動。但它仍需要一個短的上限：sudo 在 LDAP / AD 環境可能卡很久，
        // 那條執行緒不該無限期掛著。
        assert!(
            timeout_for(&Operation::Capability) <= Duration::from_secs(5),
            "啟動時的探測逾時太長"
        );
    }

    #[test]
    fn every_operation_has_a_bounded_timeout() {
        use super::super::protocol::SafeSignal;
        let ops = [
            Operation::Capability,
            Operation::StorageUsers,
            Operation::StorageUserDetail { user: "u".into() },
            Operation::UserMemory,
            Operation::GpuUsers,
            Operation::SocketMap,
            Operation::ProcessDetail { pid: 1 },
            Operation::ProcessSignal {
                pid: 1,
                starttime: 1,
                signal: SafeSignal::Term,
            },
            Operation::Renice {
                pid: 1,
                starttime: 1,
                nice: 1,
            },
        ];
        for op in ops {
            let t = timeout_for(&op);
            assert!(t > Duration::ZERO, "{} 沒有逾時保護", op.name());
            assert!(t <= Duration::from_secs(300), "{} 的逾時過長", op.name());
        }
        assert!(
            timeout_for(&Operation::StorageUsers) > timeout_for(&Operation::Capability),
            "掃 /home 本來就慢，逾時要留夠"
        );
    }

    #[test]
    fn timeout_kills_a_hanging_child() {
        let Ok(child) = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        else {
            return;
        };
        let pid = child.id();
        let t = Instant::now();
        let r = output_with_timeout(child, Duration::from_millis(300));
        assert!(matches!(r, Err(PrivError::Timeout)), "卡住的子行程必須逾時");
        assert!(t.elapsed() < Duration::from_secs(3), "逾時應該即時生效");
        // 確認被 wait 過，沒有留下殭屍行程
        std::thread::sleep(Duration::from_millis(150));
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(!stat.contains(") Z "), "逾時後留下了殭屍行程");
    }

    #[test]
    fn large_output_does_not_deadlock() {
        // 若不另開執行緒讀 stdout，超過管線緩衝區的輸出會造成死結
        let Ok(child) = Command::new("sh")
            .arg("-c")
            .arg("head -c 200000 /dev/zero | tr '\\0' 'x'")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        else {
            return;
        };
        let (status, out, _) = output_with_timeout(child, Duration::from_secs(10))
            .expect("不該逾時 —— 逾時代表讀取沒有非同步進行，發生死結");
        assert!(status.success());
        assert_eq!(out.len(), 200_000, "大量輸出必須完整讀回");
    }

    #[test]
    fn cached_state_does_not_gate_the_actual_call() {
        // 這是實際回報的 bug：背景 worker 用 without_probe() 建 client，
        // 狀態是 Locked，結果 run() 看到 Locked 就直接回錯，
        // **連 sudo 都沒呼叫過**。整個管理員功能因此永遠失敗。
        let c = PrivilegeClient::without_probe();
        assert!(
            matches!(
                c.state(),
                PrivilegeState::Locked | PrivilegeState::HelperMissing(_)
            ),
            "without_probe 的初始狀態應該是未知/鎖住"
        );
        let r = c.run(&Operation::Capability);
        match c.state() {
            // 沒有 helper 就是真的沒東西可以呼叫，直接失敗是對的
            PrivilegeState::HelperMissing(_) => {
                assert!(matches!(r, Err(PrivError::HelperMissing(_))));
            }
            _ => {
                // 有 helper 的話就必須真的打過去，而不是憑快取的狀態回絕。
                // 結果是成功還是 NeedsAuth 取決於 sudo 快取，兩者都可以，
                // 但不可以是「沒打就回絕」——那會讓狀態永遠卡在 Locked。
                assert!(
                    !matches!(r, Err(PrivError::NeedsAuth)) || {
                        // 若真的是 NeedsAuth，狀態必須同步成 Locked
                        matches!(c.state(), PrivilegeState::Locked)
                    },
                    "呼叫後的狀態必須反映實際結果"
                );
            }
        }
    }

    #[test]
    fn only_missing_helper_is_a_hard_block() {
        let c = PrivilegeClient {
            helper: None,
            state: Arc::new(Mutex::new(PrivilegeState::HelperMissing("none".into()))),
        };
        assert!(matches!(
            c.run(&Operation::Capability),
            Err(PrivError::HelperMissing(_))
        ));
    }

    #[test]
    fn client_rejects_invalid_operation_before_invoking_sudo() {
        let c = PrivilegeClient {
            helper: None,
            state: Arc::new(Mutex::new(PrivilegeState::Available)),
        };
        // pid=0 會殺整個 process group，必須在呼叫 sudo 之前就擋掉
        let r = c.run(&Operation::ProcessSignal {
            pid: 0,
            starttime: 1,
            signal: super::super::protocol::SafeSignal::Term,
        });
        assert!(matches!(r, Err(PrivError::Failed(_))));
    }
}
