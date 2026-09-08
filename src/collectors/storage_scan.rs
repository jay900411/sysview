//! 檔案系統用量掃描器（du 語意），供 per-user storage distribution 使用。
//!
//! 這是整個專案**唯一會大量走訪檔案系統**的地方，所以安全與資源上限都寫死在這裡：
//!
//! * **按需執行**，絕不進入取樣迴圈。每秒掃 `/home` 會把伺服器拖垮。
//! * 有 deadline、有 entry 上限、可取消 —— 任何一個觸發就標記為 partial 並停止。
//! * **不跟隨 symlink**（一律 `symlink_metadata`），避免無窮迴圈與越權讀取。
//!   目錄是**以 fd 為錨**走訪的：`lstat` 看到是目錄之後，用 `O_NOFOLLOW|O_DIRECTORY`
//!   開起來、`fstat` 比對同一個 (dev, ino)，之後都經 `/proc/self/fd/N/…` 讀。
//!   路徑在兩次查看之間被使用者換成 symlink（他在自己家目錄裡做得到）也進不去別的樹。
//! * **不跨檔案系統邊界**，避免把整個 NFS 或備份碟算進某個使用者頭上。
//! * hard link 以 `(device, inode)` 去重，跟 `du` 的行為一致。
//! * 用 **已配置區塊** `st_blocks × 512` 而不是 `st_size`，稀疏檔才不會被高估。
//! * 只 `lstat`，**絕不開啟檔案內容** —— socket / device / fifo 開下去會阻塞。
//! * 純 Rust 走訪，不 fork `du`，所以沒有任何指令注入面。

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// 掃描的資源上限。任何一項觸頂就停止並標記 partial。
#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    pub deadline: Duration,
    pub max_entries: u64,
    /// 目錄遞迴深度上限，防止病態結構。
    pub max_depth: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(20),
            max_entries: 3_000_000,
            max_depth: 64,
        }
    }
}

/// 掃描為什麼提前結束。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    Complete,
    Deadline,
    EntryLimit,
    Cancelled,
}

impl StopReason {
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Deadline => "timed out (partial)",
            Self::EntryLimit => "entry limit (partial)",
            Self::Cancelled => "cancelled (partial)",
        }
    }
}

/// 一次掃描的結果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    /// 已配置區塊換算的位元組數（du 語意）
    pub bytes: u64,
    /// 走訪過的項目數（檔案 + 目錄）
    pub entries: u64,
    /// 因權限不足而跳過的項目數
    pub denied: u64,
    pub stop: StopReason,
    pub elapsed_ms: u64,
    /// 第一層子目錄的個別用量，供 drill-down 用
    pub children: Vec<ChildUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildUsage {
    pub name: String,
    pub bytes: u64,
    pub entries: u64,
}

/// 可取消的旗標。UI 按 Esc 時設起來。
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

struct Walker<'a> {
    limits: ScanLimits,
    cancel: &'a Cancel,
    started: Instant,
    /// hard link 去重：同一個 (dev, ino) 只計一次，與 du 行為一致
    seen_inodes: HashSet<(u64, u64)>,
    /// 起始目錄所在的檔案系統。不同 dev 就是跨越了 mount point，停止遞迴。
    root_dev: u64,
    entries: u64,
    denied: u64,
    stop: StopReason,
}

impl Walker<'_> {
    fn budget_exhausted(&mut self) -> bool {
        if self.cancel.is_cancelled() {
            self.stop = StopReason::Cancelled;
            return true;
        }
        if self.entries >= self.limits.max_entries {
            self.stop = StopReason::EntryLimit;
            return true;
        }
        // 每 4096 個項目才看一次時鐘，避免 clock_gettime 成為瓶頸
        if self.entries.is_multiple_of(4096) && self.started.elapsed() > self.limits.deadline {
            self.stop = StopReason::Deadline;
            return true;
        }
        false
    }

    fn walk(&mut self, dir: &Path, expect: (u64, u64), depth: usize) -> u64 {
        if depth > self.limits.max_depth || self.budget_exhausted() {
            return 0;
        }
        let handle = match open_dir_checked(dir, expect) {
            Ok(h) => h,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    self.denied += 1;
                }
                return 0;
            }
        };
        let iter = match std::fs::read_dir(fd_path(&handle)) {
            Ok(i) => i,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    self.denied += 1;
                }
                return 0;
            }
        };
        let mut total = 0u64;
        for entry in iter {
            if self.budget_exhausted() {
                break;
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            // symlink_metadata：**不跟隨 symlink**。跟隨會導致無窮迴圈，
            // 也會讓一個指向 /etc 的連結被算進使用者的用量裡。
            let md = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        self.denied += 1;
                    }
                    // 檔案在 readdir 與 lstat 之間被刪掉是正常情況
                    continue;
                }
            };
            self.entries += 1;

            if md.is_dir() {
                // 不跨檔案系統邊界
                if md.dev() != self.root_dev {
                    continue;
                }
                total += self.walk(&path, (md.dev(), md.ino()), depth + 1);
                total += blocks_bytes(&md);
            } else {
                // symlink / socket / fifo / device：只算它自己的 inode 大小，
                // 絕不開啟內容（開 fifo 會直接阻塞住整個掃描）。
                if md.nlink() > 1 {
                    // hard link 只計一次
                    if !self.seen_inodes.insert((md.dev(), md.ino())) {
                        continue;
                    }
                }
                total += blocks_bytes(&md);
            }
        }
        total
    }
}

/// 開一個目錄當錨：`O_NOFOLLOW | O_DIRECTORY`，開完 `fstat` 比對 `lstat` 當時看到的
/// (dev, ino)。路徑在 `lstat` 與 `open` 之間被換成 symlink（或換成另一個目錄）
/// 就在這裡擋下 —— 不是靠「檢查得夠快」，是靠開到的東西必須就是檢查過的那個。
fn open_dir_checked(path: &Path, expect: (u64, u64)) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)?;
    let md = f.metadata()?;
    if (md.dev(), md.ino()) != expect {
        return Err(std::io::Error::other("目錄在檢查與開啟之間被換掉了"));
    }
    Ok(f)
}

/// 經由已開的 fd 定址：之後的 `read_dir` / `lstat` 都走這條，跟路徑名再無關係。
fn fd_path(f: &std::fs::File) -> std::path::PathBuf {
    use std::os::unix::io::AsRawFd;
    std::path::PathBuf::from(format!("/proc/self/fd/{}", f.as_raw_fd()))
}

/// `st_blocks` 一律以 512 位元組為單位（POSIX 規定，與檔案系統的區塊大小無關）。
fn blocks_bytes(md: &std::fs::Metadata) -> u64 {
    md.blocks().saturating_mul(512)
}

/// 掃描一個目錄，回傳 du 語意的用量與第一層子目錄明細。
///
/// `root` 由呼叫端決定，且**永遠不該來自使用者輸入** ——
/// 特權 helper 是從 password database 查出家目錄，不接受路徑參數。
pub fn scan_directory(root: &Path, limits: ScanLimits, cancel: &Cancel) -> ScanResult {
    let started = Instant::now();
    let root_md = match std::fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(_) => {
            return ScanResult {
                bytes: 0,
                entries: 0,
                denied: 1,
                stop: StopReason::Complete,
                elapsed_ms: 0,
                children: Vec::new(),
            }
        }
    };
    let mut w = Walker {
        limits,
        cancel,
        started,
        seen_inodes: HashSet::new(),
        root_dev: root_md.dev(),
        entries: 0,
        denied: 0,
        stop: StopReason::Complete,
    };

    // 先個別掃第一層子目錄，這樣才能提供 drill-down 明細
    let mut children = Vec::new();
    let mut total = blocks_bytes(&root_md);
    let mut loose_files = 0u64;

    // handle 要活到整個迴圈結束：entry.path() 都是 /proc/self/fd/<handle>/… 這種路徑
    let root_handle = open_dir_checked(root, (root_md.dev(), root_md.ino()));
    match root_handle
        .as_ref()
        .map_err(|e| std::io::Error::new(e.kind(), e.to_string()))
        .and_then(|h| std::fs::read_dir(fd_path(h)))
    {
        Ok(iter) => {
            for entry in iter {
                if w.budget_exhausted() {
                    break;
                }
                let Ok(entry) = entry else { continue };
                let path = entry.path();
                let Ok(md) = std::fs::symlink_metadata(&path) else {
                    w.denied += 1;
                    continue;
                };
                w.entries += 1;
                if md.is_dir() && md.dev() == w.root_dev {
                    let before = w.entries;
                    let sub = w.walk(&path, (md.dev(), md.ino()), 1) + blocks_bytes(&md);
                    total += sub;
                    children.push(ChildUsage {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        bytes: sub,
                        entries: w.entries - before,
                    });
                } else if !md.is_dir() {
                    if md.nlink() > 1 && !w.seen_inodes.insert((md.dev(), md.ino())) {
                        continue;
                    }
                    let b = blocks_bytes(&md);
                    total += b;
                    loose_files += b;
                }
            }
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                w.denied += 1;
            }
        }
    }

    if loose_files > 0 {
        children.push(ChildUsage {
            name: "(files)".into(),
            bytes: loose_files,
            entries: 0,
        });
    }
    children.sort_by_key(|c| std::cmp::Reverse(c.bytes));
    children.truncate(24);

    ScanResult {
        bytes: total,
        entries: w.entries,
        denied: w.denied,
        stop: w.stop,
        elapsed_ms: started.elapsed().as_millis() as u64,
        children,
    }
}

/// 使用者的家目錄用量。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserUsage {
    pub user: String,
    pub uid: u32,
    pub home: String,
    pub result: ScanResult,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn write(p: &Path, bytes: usize) {
        fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    #[test]
    fn counts_nested_files() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("a/b")).unwrap();
        write(&t.path().join("a/b/f1"), 8192);
        write(&t.path().join("a/f2"), 4096);
        let r = scan_directory(t.path(), ScanLimits::default(), &Cancel::new());
        assert!(
            r.bytes >= 12288,
            "應至少涵蓋兩個檔案的區塊，得到 {}",
            r.bytes
        );
        assert!(r.stop.is_complete());
        assert_eq!(r.denied, 0);
    }

    #[test]
    fn uses_allocated_blocks_so_sparse_files_are_not_overcounted() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("sparse");
        let f = fs::File::create(&p).unwrap();
        // 宣告 1 GB 但完全不配置區塊
        f.set_len(1024 * 1024 * 1024).unwrap();
        drop(f);
        let r = scan_directory(t.path(), ScanLimits::default(), &Cancel::new());
        assert!(
            r.bytes < 10 * 1024 * 1024,
            "稀疏檔應以已配置區塊計算，不是 st_size；得到 {} bytes",
            r.bytes
        );
    }

    #[test]
    fn deduplicates_hard_links_like_du() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("orig"), 65536);
        let single = scan_directory(t.path(), ScanLimits::default(), &Cancel::new()).bytes;
        fs::hard_link(t.path().join("orig"), t.path().join("link1")).unwrap();
        fs::hard_link(t.path().join("orig"), t.path().join("link2")).unwrap();
        let with_links = scan_directory(t.path(), ScanLimits::default(), &Cancel::new()).bytes;
        assert_eq!(with_links, single, "hard link 不可重複計算（du 語意）");
    }

    #[test]
    fn does_not_follow_symlinks() {
        let t = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(&outside.path().join("big"), 1024 * 1024);
        symlink(outside.path(), t.path().join("escape")).unwrap();
        let r = scan_directory(t.path(), ScanLimits::default(), &Cancel::new());
        assert!(
            r.bytes < 512 * 1024,
            "跟隨 symlink 會把外部資料算進來，也會導致無窮迴圈；得到 {} bytes",
            r.bytes
        );
    }

    #[test]
    fn a_directory_swapped_for_a_symlink_after_lstat_is_not_entered() {
        // lstat 說是目錄、開的時候已經是 symlink（使用者在自己家目錄裡做得到）：
        // O_NOFOLLOW 直接失敗；換成另一個目錄則 (dev, ino) 對不上。
        use std::os::unix::fs::MetadataExt;
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        let other = t.path().join("other");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(&other).unwrap();
        let rm = fs::symlink_metadata(&real).unwrap();
        assert!(open_dir_checked(&real, (rm.dev(), rm.ino())).is_ok());
        let link = t.path().join("link");
        symlink(&other, &link).unwrap();
        assert!(
            open_dir_checked(&link, (rm.dev(), rm.ino())).is_err(),
            "symlink 不能開"
        );
        let om = fs::symlink_metadata(&other).unwrap();
        assert!(
            open_dir_checked(&real, (om.dev(), om.ino())).is_err(),
            "換成別的目錄要被擋"
        );
        // 經 fd 讀到的內容跟直接讀一樣
        write(&real.join("f"), 4096);
        let h = open_dir_checked(&real, (rm.dev(), rm.ino())).unwrap();
        let names: Vec<_> = fs::read_dir(fd_path(&h))
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("f")]);
    }

    #[test]
    fn symlink_loop_terminates() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("d")).unwrap();
        symlink(t.path(), t.path().join("d/loop")).unwrap();
        // 不跟隨 symlink 的話這裡必須立刻結束
        let r = scan_directory(t.path(), ScanLimits::default(), &Cancel::new());
        assert!(r.stop.is_complete(), "symlink 迴圈不該耗盡預算");
    }

    #[test]
    fn respects_entry_limit_and_marks_partial() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..200 {
            write(&t.path().join(format!("f{i}")), 16);
        }
        let limits = ScanLimits {
            max_entries: 20,
            ..Default::default()
        };
        let r = scan_directory(t.path(), limits, &Cancel::new());
        assert_eq!(r.stop, StopReason::EntryLimit);
        assert!(
            !r.stop.is_complete(),
            "超出上限必須標記為 partial，不能假裝掃完了"
        );
    }

    #[test]
    fn respects_cancellation() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..100 {
            write(&t.path().join(format!("f{i}")), 16);
        }
        let c = Cancel::new();
        c.cancel();
        let r = scan_directory(t.path(), ScanLimits::default(), &c);
        assert_eq!(r.stop, StopReason::Cancelled);
    }

    #[test]
    fn respects_deadline() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..50 {
            let d = t.path().join(format!("d{i}"));
            fs::create_dir(&d).unwrap();
            for j in 0..200 {
                write(&d.join(format!("f{j}")), 8);
            }
        }
        let limits = ScanLimits {
            deadline: Duration::from_nanos(1),
            ..Default::default()
        };
        let r = scan_directory(t.path(), limits, &Cancel::new());
        assert!(!r.stop.is_complete(), "已逾時就必須標記 partial");
    }

    #[test]
    fn respects_max_depth() {
        let t = tempfile::tempdir().unwrap();
        let mut p = t.path().to_path_buf();
        for i in 0..30 {
            p = p.join(format!("d{i}"));
        }
        fs::create_dir_all(&p).unwrap();
        write(&p.join("deep"), 4096);
        let limits = ScanLimits {
            max_depth: 3,
            ..Default::default()
        };
        let r = scan_directory(t.path(), limits, &Cancel::new());
        // 沒有 panic、沒有無窮遞迴就算通過
        assert!(r.entries > 0);
    }

    #[test]
    fn reports_children_sorted_by_size() {
        let t = tempfile::tempdir().unwrap();
        for (name, size) in [("small", 4096), ("big", 262144), ("mid", 65536)] {
            let d = t.path().join(name);
            fs::create_dir(&d).unwrap();
            write(&d.join("f"), size);
        }
        let r = scan_directory(t.path(), ScanLimits::default(), &Cancel::new());
        let names: Vec<&str> = r.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names.first(), Some(&"big"), "子目錄應依用量由大到小排序");
        assert!(names.contains(&"small"));
    }

    #[test]
    fn missing_root_returns_empty_not_panic() {
        let r = scan_directory(
            Path::new("/nonexistent/path/for/test"),
            ScanLimits::default(),
            &Cancel::new(),
        );
        assert_eq!(r.bytes, 0);
        assert_eq!(r.denied, 1);
    }

    #[test]
    fn permission_denied_is_counted_not_fatal() {
        let t = tempfile::tempdir().unwrap();
        let locked = t.path().join("locked");
        fs::create_dir(&locked).unwrap();
        write(&locked.join("secret"), 4096);
        // 移除自己的讀取權限（root 執行測試時無效，所以有條件斷言）
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
        fs::set_permissions(&locked, perms).unwrap();
        let r = scan_directory(t.path(), ScanLimits::default(), &Cancel::new());
        if unsafe { libc::geteuid() } != 0 {
            assert!(r.denied > 0, "無權限的目錄應被計數而不是中斷掃描");
        }
        assert!(r.stop.is_complete(), "權限不足不是致命錯誤");
        // 還原權限讓 tempdir 能清乾淨
        let mut perms = fs::metadata(&locked).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        let _ = fs::set_permissions(&locked, perms);
    }

    #[test]
    fn does_not_open_fifos() {
        let t = tempfile::tempdir().unwrap();
        let fifo = t.path().join("pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // 建立一個沒有寫入端的 FIFO：如果掃描器去 open 它就會永遠卡住
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let r = scan_directory(t.path(), ScanLimits::default(), &Cancel::new());
        assert!(r.stop.is_complete(), "掃描器絕不可開啟 FIFO 內容");
        assert!(r.entries >= 1);
    }
}
