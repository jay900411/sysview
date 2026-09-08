//! 讀 `/proc` 與 `/sys` 的低階工具。
//!
//! 這一層刻意保持「看得懂」：每個數字都能追到它是從哪個檔案的哪一欄來的，
//! 這是 sysview 的教育價值所在，所以不用 `sysinfo` 之類的抽象把來源蓋掉。

use crate::error::Unavailable;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::Instant;

/// 讀整個小檔案。
///
/// `/proc` 底下的檔案 `stat()` 出來大小是 0，所以不能用 `fs::read_to_string` 預先配置，
/// 必須一路 read 到 EOF。這裡重複使用呼叫端的緩衝區，避免每次取樣都重新配置。
pub fn read_into(path: impl AsRef<Path>, buf: &mut String) -> Result<(), Unavailable> {
    buf.clear();
    let path = path.as_ref();
    let mut f = File::open(path).map_err(|e| Unavailable::from_io(&e))?;
    match f.read_to_string(buf) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            // 少數 sysfs 節點含非 UTF-8 位元組，改用 lossy 讀。
            buf.clear();
            let mut raw = Vec::new();
            let mut f2 = File::open(path).map_err(|e| Unavailable::from_io(&e))?;
            f2.read_to_end(&mut raw)
                .map_err(|e| Unavailable::from_io(&e))?;
            buf.push_str(&String::from_utf8_lossy(&raw));
            Ok(())
        }
        Err(e) => Err(Unavailable::from_io(&e)),
    }
}

/// 讀成新的 `String`（不需要重用緩衝區時用這個）。
pub fn read_string(path: impl AsRef<Path>) -> Result<String, Unavailable> {
    let mut s = String::new();
    read_into(path, &mut s)?;
    Ok(s)
}

/// 讀成去掉前後空白的字串。
pub fn read_trimmed(path: impl AsRef<Path>) -> Result<String, Unavailable> {
    Ok(read_string(path)?.trim().to_owned())
}

/// 讀一個整數（sysfs 大量使用這種單值檔案）。
pub fn read_i64(path: impl AsRef<Path>) -> Result<i64, Unavailable> {
    let s = read_trimmed(&path)?;
    s.parse::<i64>()
        .map_err(|_| Unavailable::Malformed(format!("expected integer, got {s:?}")))
}

/// 讀整數，失敗就回 `None`（大量 sysfs 節點是選配的）。
pub fn read_i64_opt(path: impl AsRef<Path>) -> Option<i64> {
    read_i64(path).ok()
}

/// 取第 `n` 個以空白分隔的欄位並 parse。
pub fn field<T: std::str::FromStr>(line: &str, n: usize) -> Option<T> {
    line.split_ascii_whitespace().nth(n)?.parse().ok()
}

/// 單調遞增計數器的差分器：把「累計值」換算成「每秒速率」。
///
/// 開機以來的累計值可能因為 32 位元欄位而回捲（`/proc/net/dev` 在舊核心上會），
/// 回捲時回 `None` 而不是回傳一個荒謬的巨大速率。
#[derive(Debug, Clone, Default)]
pub struct Counter {
    prev: Option<(u64, Instant)>,
}

impl Counter {
    pub fn new() -> Self {
        Self { prev: None }
    }

    /// 餵入新的累計值，回傳每秒速率。第一次呼叫必然是 `None`（沒有基準點）。
    pub fn update(&mut self, value: u64, now: Instant) -> Option<f64> {
        let out = match self.prev {
            Some((pv, pt)) if value >= pv => {
                let dt = now.duration_since(pt).as_secs_f64();
                if dt > 1e-6 {
                    Some((value - pv) as f64 / dt)
                } else {
                    None
                }
            }
            // 回捲或重設：重新取基準，這一輪不報數字。
            _ => None,
        };
        self.prev = Some((value, now));
        out
    }

    pub fn reset(&mut self) {
        self.prev = None;
    }
}

/// 把 jiffies 型的 CPU 時間差換算成百分比使用率。
#[derive(Debug, Clone, Default)]
pub struct CpuDelta {
    prev_total: u64,
    prev_idle: u64,
    primed: bool,
}

impl CpuDelta {
    pub fn update(&mut self, total: u64, idle: u64) -> Option<f64> {
        let out = if self.primed && total > self.prev_total {
            let dt = total - self.prev_total;
            let di = idle.saturating_sub(self.prev_idle);
            Some(((dt - di.min(dt)) as f64 / dt as f64 * 100.0).clamp(0.0, 100.0))
        } else {
            None
        };
        self.prev_total = total;
        self.prev_idle = idle;
        self.primed = true;
        out
    }
}

/// 系統常數，只查一次。
pub fn clock_ticks() -> u64 {
    static ONCE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if v > 0 {
            v as u64
        } else {
            100
        }
    })
}

pub fn page_size() -> u64 {
    static ONCE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v > 0 {
            v as u64
        } else {
            4096
        }
    })
}

/// uid → 使用者名稱。查一次就快取（`getpwuid` 在 LDAP/AD 環境下可能很慢）。
pub fn username(uid: u32) -> String {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: std::sync::OnceLock<Mutex<HashMap<u32, String>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(map) = cache.lock() {
        if let Some(n) = map.get(&uid) {
            return n.clone();
        }
    }
    let name = lookup_uid(uid).unwrap_or_else(|| uid.to_string());
    if let Ok(mut map) = cache.lock() {
        map.insert(uid, name.clone());
    }
    name
}

fn lookup_uid(uid: u32) -> Option<String> {
    // SAFETY: getpwuid 回傳指向靜態緩衝區的指標；我們立刻複製出字串，
    // 且在同一執行緒內不會有第二次呼叫介入。
    unsafe {
        let pw = libc::getpwuid(uid as libc::uid_t);
        if pw.is_null() {
            return None;
        }
        let name = (*pw).pw_name;
        if name.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(name)
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

/// 讓 `SIGPIPE` 回到預設行為（安靜地終止行程），而不是被 Rust 執行期忽略。
///
/// Rust 啟動時會把 SIGPIPE 設成 `SIG_IGN`，於是「寫進沒人在讀的管線」不再是
/// 訊號而是 `EPIPE` 錯誤 —— 而 `println!` 遇到寫入錯誤就 panic：
///
/// ```text
/// $ sysview --json | jq .cpu
/// thread 'main' panicked: failed printing to stdout: Broken pipe (os error 32)
/// ```
///
/// 對一個設計成要放進管線的 CLI 來說那是錯的：`head`、`jq`、`less` 提早離開
/// 是**正常**的用法，正確的反應是安靜地跟著結束，就像 `cat` 或 `ls`。
/// README 自己就寫著 `sysview --json | jq .`。
///
/// 必須在**建立任何執行緒之前**、程式最開頭呼叫一次。
pub fn restore_default_sigpipe() {
    // SAFETY: 把 SIGPIPE 設回 SIG_DFL 是 async-signal-safe 的，而且這裡在
    // 任何執行緒存在之前執行，不會和別人競爭訊號設定。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// 目前程序的有效 uid。
pub fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

/// 目前程序的真實 uid。
pub fn real_uid() -> u32 {
    unsafe { libc::getuid() }
}
