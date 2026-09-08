//! 恐龍彩蛋的排行榜 —— 這台機器所有人共用，但**沒有任何特權**。
//!
//! ## 儲存方式
//!
//! 每個使用者一個自己的檔（`<uid>.json`，0644），放在一個共用目錄裡；
//! 榜是把目錄裡所有人的檔**合併**出來的。為什麼不是一個共用的檔：那個檔
//! 必須 world-writable，任何人都能改掉別人的紀錄。每人一檔、加上 sticky
//! bit 的目錄（跟 `/tmp` 一樣，`1777`）：你只能寫自己的，別人的只能讀。
//!
//! 共用目錄由管理員在安裝時建（`install -d -m 1777 /var/lib/sysview/dino`），
//! sysview 自己**不會**建它 —— 建它需要 root，而 sysview 永遠不以 root 跑。
//! 目錄不存在或不能寫，就退回每人自己的 XDG data 目錄：榜上只有自己，
//! 畫面會說「共用榜未啟用」。`SYSVIEW_DINO_SCORES` 可以指定別的目錄。
//!
//! ## 規則（使用者訂的）
//!
//! * 死掉時分數**嚴格大於**第五名（或榜上不到五筆）才問名字。
//! * 同名取最高：舊的比新的高就不記，新的比舊的高就蓋掉舊的。
//! * 不填名字就不記。
//!
//! ## 這是遊戲的榜，不是身分或稽核資料
//!
//! 分數是在使用者自己的行程裡算出來的：sysview 能寫的，使用者用編輯器也
//! 能寫。真正的防作弊要把算分數的程式放在使用者動不了的權限下（setgid
//! 的遊戲、daemon），這個專案明文不做。所以這裡做的是「讓作弊看得見」：
//!
//! * **帳號**（無法偽造）：每筆紀錄顯示它來自哪個帳號 —— 檔的 owner，
//!   kernel 記的。你可以在自己的檔裡寫別人的名字，榜上會顯示是你貼的。
//! * **標籤**（嚇阻）：每筆紀錄帶一個 keyed hash（綁 uid、名字、分數、
//!   時間）。用編輯器改過的紀錄標籤對不上，**靜默丟掉** —— 不排名、
//!   不顯示、不計數，改的人不會得到「失敗了」的提示。金鑰在原始碼裡
//!   （見 [`KEY`]），看過的人算得出合法標籤；擋的是順手改 JSON，不是有心人。
//!
//! 別人的檔一律是**不可信的輸入**：只讀一般檔（不跟 symlink、不碰 FIFO）、
//! 檔名的 uid 必須等於檔的 owner、大小有上限、JSON 壞了整個跳過、名字
//! 清掉控制字元並截短。除了自己的那個檔，這裡不寫任何東西。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 榜上顯示幾名。
pub const TOP: usize = 5;
/// 名字最多幾格寬（全形算兩格）。
pub const NAME_WIDTH: usize = 12;
/// 每個人自己的檔最多留幾筆（每個名字一筆）。
const KEEP_PER_USER: usize = 50;
/// 別人的檔超過這個大小就不讀 —— 這是唯一會讀別人東西的地方。
const MAX_FILE: u64 = 64 * 1024;
/// 預設的共用目錄。
pub const SHARED_DIR: &str = "/var/lib/sysview/dino";
/// 指定別的共用目錄用的環境變數。
pub const ENV_DIR: &str = "SYSVIEW_DINO_SCORES";

/// 標籤的金鑰。
///
/// 好吧，算你厲害找到這裡了。有了這組金鑰，你可以自己算出合法的標籤、
/// 在榜上放任何分數 —— 我們知道，這裡不防真正有心的人；這只是一個
/// 死掉之後填名字的小遊戲。但拜託別改規則，也別把別人的名字擠掉，
/// 哈哈哈。（榜上永遠看得到那筆紀錄是從哪個帳號來的，那個改不了。）
const KEY: [u8; 16] = *b"sysview-dino-1.0";

/// 檔案裡的一筆（跟 [`Entry`] 差在多了標籤、沒有 uid —— uid 是檔的 owner）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    name: String,
    score: u32,
    #[serde(default)]
    at: u64,
    #[serde(default)]
    tag: String,
}

/// 榜上的一筆（已驗證）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub score: u32,
    /// 記錄時間（epoch 秒）。同分時先到的排前面。
    pub at: u64,
    /// 來自哪個帳號 —— 檔的 owner，不是檔裡寫的。
    pub uid: u32,
}

/// 合併後的榜：同名只留最高，分數高的在前。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Board {
    entries: Vec<Entry>,
}

impl Board {
    /// 把任何來源的紀錄合併成一個榜。
    pub fn merge(entries: impl IntoIterator<Item = Entry>) -> Self {
        let mut out: Vec<Entry> = Vec::new();
        for mut e in entries {
            e.name = sanitize_name(&e.name);
            if e.name.is_empty() || e.score == 0 {
                continue;
            }
            match out.iter_mut().find(|o| o.name == e.name) {
                Some(o) => {
                    if e.score > o.score || (e.score == o.score && e.at < o.at) {
                        *o = e;
                    }
                }
                None => out.push(e),
            }
        }
        out.sort_by(|a, b| b.score.cmp(&a.score).then(a.at.cmp(&b.at)));
        Self { entries: out }
    }

    /// 前幾名。
    pub fn top(&self) -> &[Entry] {
        &self.entries[..self.entries.len().min(TOP)]
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 這個分數進得了榜嗎：**嚴格**大於第五名，或榜上還不到五筆。0 分不算。
    pub fn qualifies(&self, score: u32) -> bool {
        if score == 0 {
            return false;
        }
        match self.entries.get(TOP - 1) {
            Some(fifth) => score > fifth.score,
            None => true,
        }
    }

    /// 這個名字目前的最高分。
    pub fn best_of(&self, name: &str) -> Option<u32> {
        let name = sanitize_name(name);
        self.entries
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.score)
    }

    /// 這個名字現在第幾名（從 1 數）。
    pub fn rank_of(&self, name: &str) -> Option<usize> {
        let name = sanitize_name(name);
        self.entries
            .iter()
            .position(|e| e.name == name)
            .map(|i| i + 1)
    }
}

/// 把名字變成可以放上榜的樣子：去掉控制字元、多餘空白，太長就截短。
///
/// 別人檔裡的名字也經過這裡 —— 一個 ANSI 跳脫序列就能把終端機畫壞。
pub fn sanitize_name(raw: &str) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut out = String::new();
    let mut width = 0usize;
    let mut pending_space = false;
    for c in raw.chars() {
        // 控制字元一律丟掉 —— Tab、換行也算，不會變成分隔空白（名字裡本來
        // 就不該有它們）；全形空白不是控制字元，照空白處理。
        if c.is_control() {
            continue;
        }
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        let w = c.width().unwrap_or(0);
        if w == 0 {
            // 組合字元、零寬字元：不佔格但能把畫面弄亂，跳過
            continue;
        }
        let extra = usize::from(pending_space);
        if width + extra + w > NAME_WIDTH {
            break;
        }
        if pending_space {
            out.push(' ');
            width += 1;
            pending_space = false;
        }
        out.push(c);
        width += w;
    }
    out
}

/// 一筆紀錄的標籤：SipHash-2-4，綁 uid、名字、分數、時間。
pub fn tag(uid: u32, name: &str, score: u32, at: u64) -> String {
    let msg = format!("{uid}\n{name}\n{score}\n{at}");
    format!("{:016x}", siphash24(&KEY, msg.as_bytes()))
}

fn verify(uid: u32, s: &Stored) -> Option<Entry> {
    let name = sanitize_name(&s.name);
    if name.is_empty() || s.score == 0 {
        return None;
    }
    // 標籤是對清理過的名字算的：檔裡有人塞了控制字元，清完就對不上 ——
    // 那本來就不是 sysview 寫的。
    if s.tag != tag(uid, &name, s.score, s.at) {
        return None;
    }
    Some(Entry {
        name,
        score: s.score,
        at: s.at,
        uid,
    })
}

/// 記錄的結果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// 記下來了，現在第幾名（合併後的榜，從 1 數）。
    Recorded { rank: usize },
    /// 榜上同名的紀錄已經一樣高或更高，沒有記。
    NotHigher { existing: u32 },
}

/// 榜存在哪裡。
#[derive(Clone, Debug)]
pub struct Store {
    /// 共用目錄。`None` = 沒有共用榜，只有自己的檔。
    shared: Option<PathBuf>,
    /// 自己的檔。
    own: PathBuf,
    /// 自己的 uid：寫檔時標籤綁它，讀自己的檔時也用它驗。
    uid: u32,
}

impl Store {
    /// 依環境決定放哪：`SYSVIEW_DINO_SCORES` → 預設共用目錄 → 自己的 XDG data。
    ///
    /// 這裡只做 `stat` 與 `access`，不建任何目錄。
    pub fn locate() -> Self {
        let uid = crate::collectors::util::real_uid();
        let candidate = std::env::var_os(ENV_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(SHARED_DIR));
        if dir_writable(&candidate) {
            return Self {
                own: candidate.join(format!("{uid}.json")),
                shared: Some(candidate),
                uid,
            };
        }
        Self {
            shared: None,
            own: xdg_data_home().join("sysview").join("dino-scores.json"),
            uid,
        }
    }

    /// 指定位置（測試用）。
    pub fn with_paths(shared: Option<PathBuf>, own: PathBuf, uid: u32) -> Self {
        Self { shared, own, uid }
    }

    /// 有共用榜嗎（沒有的話榜上只有自己）。
    pub fn is_shared(&self) -> bool {
        self.shared.is_some()
    }

    /// 榜放在哪（給畫面說明用）。
    pub fn location(&self) -> &Path {
        self.shared.as_deref().unwrap_or(&self.own)
    }

    /// 讀出合併後的榜。讀不到就是空榜，不是錯誤。
    pub fn load(&self) -> Board {
        let mut all: Vec<Entry> = Vec::new();
        match &self.shared {
            Some(dir) => {
                if let Ok(rd) = std::fs::read_dir(dir) {
                    for entry in rd.flatten() {
                        let path = entry.path();
                        let Some(uid) = score_file_owner(&path) else {
                            continue;
                        };
                        all.extend(read_file(&path).into_iter().filter_map(|s| verify(uid, &s)));
                    }
                }
            }
            None => {
                all.extend(
                    read_file(&self.own)
                        .into_iter()
                        .filter_map(|s| verify(self.uid, &s)),
                );
            }
        }
        Board::merge(all)
    }

    /// 自己檔裡（驗證過的）紀錄，每個名字一筆。
    fn own_entries(&self) -> Vec<Entry> {
        let mine = read_file(&self.own)
            .into_iter()
            .filter_map(|s| verify(self.uid, &s));
        Board::merge(mine).entries
    }

    /// 記一筆。名字先經過 [`sanitize_name`]；空的就是呼叫端的錯。
    ///
    /// 同名取最高：合併後的榜上這個名字已經一樣高或更高，就不記
    /// （回 [`Verdict::NotHigher`]）。只寫自己的檔，先寫暫存檔再 rename，
    /// 模式 0644 —— 別人要讀得到才有共用榜；umask 再嚴也不會變成只有自己看得到。
    pub fn record(&self, name: &str, score: u32) -> std::io::Result<Verdict> {
        let name = sanitize_name(name);
        if name.is_empty() || score == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "名字或分數是空的",
            ));
        }
        let board = self.load();
        if let Some(existing) = board.best_of(&name) {
            if existing >= score {
                return Ok(Verdict::NotHigher { existing });
            }
        }
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut mine = self.own_entries();
        mine.retain(|e| e.name != name);
        mine.push(Entry {
            name: name.clone(),
            score,
            at,
            uid: self.uid,
        });
        mine.sort_by(|a, b| b.score.cmp(&a.score).then(a.at.cmp(&b.at)));
        mine.truncate(KEEP_PER_USER);
        let stored: Vec<Stored> = mine
            .iter()
            .map(|e| Stored {
                name: e.name.clone(),
                score: e.score,
                at: e.at,
                tag: tag(self.uid, &e.name, e.score, e.at),
            })
            .collect();
        write_file(&self.own, &stored)?;
        let rank = self.load().rank_of(&name).unwrap_or(usize::MAX);
        Ok(Verdict::Recorded { rank })
    }
}

/// 共用目錄裡一個檔算不算數，算的話回它的 uid。
///
/// 只收 `<uid>.json` 的**一般檔**（symlink、FIFO、目錄都不算 —— 跟著
/// symlink 讀，別人就能讓你去讀任何檔，FIFO 更會把整個程式卡住），
/// 而且檔的 owner 必須就是檔名那個 uid：不然先搶建別人的檔名塞假資料就成了。
fn score_file_owner(path: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".json")?;
    if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let uid: u32 = stem.parse().ok()?;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE || meta.uid() != uid {
        return None;
    }
    Some(uid)
}

/// 讀一個檔裡的紀錄（還沒驗證）。任何問題都當作「這個檔沒有紀錄」。
fn read_file(path: &Path) -> Vec<Stored> {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    // 大小在 stat 之後可能變了：讀的時候再擋一次
    let mut buf = Vec::new();
    if f.take(MAX_FILE + 1).read_to_end(&mut buf).is_err() || buf.len() as u64 > MAX_FILE {
        return Vec::new();
    }
    serde_json::from_slice::<Vec<Stored>>(&buf).unwrap_or_default()
}

fn write_file(path: &Path, entries: &[Stored]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        // 共用目錄不會在這裡被建出來：它存在才會走到 shared 模式。
        // 這裡建的只會是自己的 XDG data 目錄。
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    // 暫存檔用 O_EXCL 建：共用目錄是 1777，別人可以事先在我們會用的名字上放一個
    // symlink 或一般檔等我們去寫。O_EXCL 遇到任何已存在的東西（包括 symlink）都是
    // 失敗而不是跟過去；名字撞了就換下一個序號，永遠不覆蓋已存在的檔。核心的
    // protected_symlinks / protected_regular 在多數發行版預設也會擋這種事，
    // 但那是別人的設定，不能當自己的防線。
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scores");
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let pid = std::process::id();
    let (tmp, mut f) = (0..64)
        .map(|n| dir.join(format!("{stem}.tmp-{pid}-{n}")))
        .find_map(|tmp| {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o644)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&tmp)
            {
                Ok(f) => Some(Ok((tmp, f))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(e) => Some(Err(e)),
            }
        })
        .unwrap_or_else(|| Err(std::io::Error::other("暫存檔名全部被佔用")))?;
    let written = (|| {
        f.write_all(&serde_json::to_vec_pretty(entries)?)?;
        f.sync_all()?;
        // create 的 mode 會被 umask 遮掉：透過 fd 明確設一次（不經路徑，換不掉）
        f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// 目錄存在而且我們寫得進去。只用 `access(2)`，不試寫。
fn dir_writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    if !dir.is_dir() {
        return false;
    }
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c` 是合法的 NUL 結尾字串；access 不會保留指標。
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

fn xdg_data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(".local")
                .join("share")
        })
}

/// SipHash-2-4（Aumasson & Bernstein）。自己寫是為了不為一個彩蛋多拉一個
/// 密碼學相依；它只是標籤，不是安全邊界（見 [`KEY`] 旁邊那段話）。
fn siphash24(key: &[u8; 16], msg: &[u8]) -> u64 {
    let k0 = u64::from_le_bytes(key[..8].try_into().unwrap());
    let k1 = u64::from_le_bytes(key[8..].try_into().unwrap());
    let mut v0 = k0 ^ 0x736f_6d65_7073_6575;
    let mut v1 = k1 ^ 0x646f_7261_6e64_6f6d;
    let mut v2 = k0 ^ 0x6c79_6765_6e65_7261;
    let mut v3 = k1 ^ 0x7465_6462_7974_6573;
    macro_rules! round {
        () => {
            v0 = v0.wrapping_add(v1);
            v1 = v1.rotate_left(13);
            v1 ^= v0;
            v0 = v0.rotate_left(32);
            v2 = v2.wrapping_add(v3);
            v3 = v3.rotate_left(16);
            v3 ^= v2;
            v0 = v0.wrapping_add(v3);
            v3 = v3.rotate_left(21);
            v3 ^= v0;
            v2 = v2.wrapping_add(v1);
            v1 = v1.rotate_left(17);
            v1 ^= v2;
            v2 = v2.rotate_left(32);
        };
    }
    let (chunks, rest) = msg.as_chunks::<8>();
    for c in chunks {
        let m = u64::from_le_bytes(*c);
        v3 ^= m;
        round!();
        round!();
        v0 ^= m;
    }
    let mut last = (msg.len() as u64 & 0xff) << 56;
    for (i, b) in rest.iter().enumerate() {
        last |= u64::from(*b) << (8 * i);
    }
    v3 ^= last;
    round!();
    round!();
    v0 ^= last;
    v2 ^= 0xff;
    for _ in 0..4 {
        round!();
    }
    v0 ^ v1 ^ v2 ^ v3
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "sysview-scoreboard-{}-{}-{}",
            std::process::id(),
            tag,
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn me() -> u32 {
        crate::collectors::util::real_uid()
    }

    fn entry(name: &str, score: u32, at: u64, uid: u32) -> Entry {
        Entry {
            name: name.into(),
            score,
            at,
            uid,
        }
    }

    /// 一個「別人」寫的檔：內容合法、標籤合法，但 owner 是我（測試沒有 root）。
    fn stored(uid: u32, name: &str, score: u32, at: u64) -> Stored {
        Stored {
            name: name.into(),
            score,
            at,
            tag: tag(uid, name, score, at),
        }
    }

    #[test]
    fn siphash_matches_the_reference_vectors() {
        let key: [u8; 16] = core::array::from_fn(|i| i as u8);
        let msg: Vec<u8> = (0u8..15).collect();
        assert_eq!(siphash24(&key, &[]), 0x726f_db47_dd0e_0e31);
        assert_eq!(siphash24(&key, &msg), 0xa129_ca61_49be_45e5);
    }

    #[test]
    fn the_tag_binds_every_field_including_the_account() {
        let t = tag(1000, "jay", 120, 7);
        assert_eq!(t.len(), 16);
        assert_ne!(t, tag(1001, "jay", 120, 7), "換帳號要變");
        assert_ne!(t, tag(1000, "Ada", 120, 7), "換名字要變");
        assert_ne!(t, tag(1000, "jay", 121, 7), "換分數要變");
        assert_ne!(t, tag(1000, "jay", 120, 8), "換時間要變");
        assert_eq!(t, tag(1000, "jay", 120, 7), "同樣的東西永遠同一個標籤");
    }

    #[test]
    fn the_board_keeps_the_highest_score_per_name_and_sorts_by_score() {
        let b = Board::merge([
            entry("jay", 100, 5, 1),
            entry("amy", 300, 1, 2),
            entry("jay", 250, 9, 3),
            entry("bob", 250, 2, 4),
            entry("zero", 0, 1, 5),
        ]);
        let names: Vec<(&str, u32, u32)> = b
            .top()
            .iter()
            .map(|e| (e.name.as_str(), e.score, e.uid))
            .collect();
        // 同分先到的在前；0 分不上榜；jay 只留 250（來自 uid 3）
        assert_eq!(
            names,
            vec![("amy", 300, 2), ("bob", 250, 4), ("jay", 250, 3)]
        );
    }

    #[test]
    fn qualifying_is_strictly_above_fifth_place() {
        let empty = Board::default();
        assert!(empty.qualifies(1));
        assert!(!empty.qualifies(0), "0 分不問名字");
        let b = Board::merge((0..5).map(|i| entry(&format!("u{i}"), 100 + i, 0, 1)));
        assert!(!b.qualifies(100), "跟第五名同分不算");
        assert!(b.qualifies(101));
        let four = Board::merge((0..4).map(|i| entry(&format!("u{i}"), 100 + i, 0, 1)));
        assert!(four.qualifies(1), "不到五筆時任何分數都進得去");
    }

    #[test]
    fn names_are_sanitized_before_they_reach_the_screen() {
        assert_eq!(sanitize_name("  jay   wu "), "jay wu");
        assert_eq!(sanitize_name("\x1b[31mred\x1b[0m"), "[31mred[0m");
        // Tab 是控制字元，在空白判斷之前就被丟掉了 —— 不會變成分隔空白。
        // 零寬字元不佔格但能把畫面弄亂，也丟掉。
        assert_eq!(sanitize_name("a\u{200b}b\tc"), "abc");
        assert_eq!(sanitize_name("jay\u{3000}wu"), "jay wu", "全形空白也是空白");
        assert_eq!(
            sanitize_name("abcdefghijklmnop"),
            "abcdefghijkl",
            "太長要截"
        );
        assert_eq!(
            sanitize_name("測試測試測試測試"),
            "測試測試測試",
            "全形算兩格"
        );
        assert_eq!(sanitize_name("   "), "");
    }

    #[test]
    fn recording_writes_only_my_own_file_with_mode_0644_and_a_valid_tag() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("own");
        let own = dir.join(format!("{}.json", me()));
        let store = Store::with_paths(Some(dir.clone()), own.clone(), me());
        assert_eq!(
            store.record("jay", 120).unwrap(),
            Verdict::Recorded { rank: 1 }
        );
        let mode = std::fs::metadata(&own).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "別人要讀得到才有共用榜");
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "只能多出自己的檔");
        let raw = read_file(&own);
        assert_eq!(raw[0].tag, tag(me(), "jay", 120, raw[0].at));
        let top = store.load();
        assert_eq!((top.top()[0].score, top.top()[0].uid), (120, me()));
    }

    #[test]
    fn a_symlink_planted_at_the_temp_name_is_never_followed() {
        // 1777 的共用目錄裡，別人可以先在我們會用的暫存檔名上放一個 symlink。
        // O_EXCL 讓它只是「這個名字不能用」：換下一個；symlink 指到的檔一根毛都不能少。
        let dir = scratch("planted");
        let canary = dir.join("canary.txt");
        std::fs::write(&canary, b"do not touch").unwrap();
        let own = dir.join(format!("{}.json", me()));
        let planted = dir.join(format!("{}.tmp-{}-0", me(), std::process::id()));
        std::os::unix::fs::symlink(&canary, &planted).unwrap();
        let store = Store::with_paths(Some(dir.clone()), own.clone(), me());
        assert!(matches!(
            store.record("ada", 300).unwrap(),
            Verdict::Recorded { .. }
        ));
        assert_eq!(std::fs::read(&canary).unwrap(), b"do not touch");
        assert!(
            std::fs::symlink_metadata(&planted)
                .unwrap()
                .file_type()
                .is_symlink(),
            "被種的 symlink 原封不動"
        );
        assert!(std::fs::symlink_metadata(&own).unwrap().is_file());
        assert_eq!(store.load().rank_of("ada"), Some(1));
        let leftovers = std::fs::read_dir(&dir).unwrap().flatten().count();
        assert_eq!(leftovers, 3, "canary、symlink、自己的檔，沒有多餘的暫存檔");
    }

    #[test]
    fn the_same_name_only_moves_up() {
        let dir = scratch("same");
        let store = Store::with_paths(Some(dir.clone()), dir.join(format!("{}.json", me())), me());
        store.record("jay", 200).unwrap();
        assert_eq!(
            store.record("jay", 150).unwrap(),
            Verdict::NotHigher { existing: 200 },
            "舊的比新的高：不記"
        );
        assert_eq!(store.load().best_of("jay"), Some(200));
        assert_eq!(
            store.record("jay", 260).unwrap(),
            Verdict::Recorded { rank: 1 }
        );
        let mine = store.own_entries();
        assert_eq!(mine.len(), 1, "同一個名字只留一筆");
        assert_eq!(mine[0].score, 260, "新的比舊的高：蓋掉");
    }

    #[test]
    fn a_hand_edited_record_disappears_silently() {
        let dir = scratch("edited");
        let own = dir.join(format!("{}.json", me()));
        let store = Store::with_paths(Some(dir.clone()), own.clone(), me());
        store.record("jay", 120).unwrap();
        store.record("amy", 90).unwrap();
        // 用編輯器把 jay 改成 99999
        let text = std::fs::read_to_string(&own)
            .unwrap()
            .replace("120", "99999");
        std::fs::write(&own, text).unwrap();
        let b = store.load();
        let names: Vec<(&str, u32)> = b.top().iter().map(|e| (e.name.as_str(), e.score)).collect();
        assert_eq!(
            names,
            vec![("amy", 90)],
            "改過的那筆不排名、不顯示；其他筆照舊"
        );
        // 也不會擋住別人：99999 不算數，所以 100 分照樣進得了榜
        assert!(b.qualifies(100));
    }

    #[test]
    fn hostile_files_in_the_shared_dir_are_ignored() {
        let dir = scratch("hostile");
        let m = me();
        // 別人的 uid 檔名、但 owner 是我 → 搶建別人的檔名，不算
        std::fs::write(
            dir.join(format!("{}.json", m + 1)),
            serde_json::to_vec(&[stored(m + 1, "fake", 9999, 0)]).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("notes.json"), b"[]").unwrap();
        std::fs::write(dir.join("2000.json"), b"{not json").unwrap();
        std::fs::write(dir.join("3000.json"), vec![b' '; (MAX_FILE + 1) as usize]).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", dir.join("4000.json")).unwrap();
        // 我自己的檔：一筆合法、一筆標籤壞掉、一筆名字塞了跳脫序列（標籤對清乾淨的名字算，對不上）
        let good = stored(m, "ok", 77, 1);
        let mut bad = stored(m, "ok2", 500, 1);
        bad.tag = "0000000000000000".into();
        let ugly = Stored {
            name: "x\x1b[2J".into(),
            score: 60,
            at: 1,
            tag: tag(m, "x\x1b[2J", 60, 1),
        };
        std::fs::write(
            dir.join(format!("{m}.json")),
            serde_json::to_vec(&[good, bad, ugly]).unwrap(),
        )
        .unwrap();
        let store = Store::with_paths(Some(dir.clone()), dir.join(format!("{m}.json")), m);
        let b = store.load();
        let names: Vec<(&str, u32)> = b.top().iter().map(|e| (e.name.as_str(), e.score)).collect();
        assert_eq!(names, vec![("ok", 77)]);
    }

    #[test]
    fn verified_records_from_other_accounts_merge_and_show_their_account() {
        // 檔案層在沒有 root 的測試裡做不出「別人的檔」（owner 改不了），
        // 合併的規則就直接用驗證過的紀錄測。
        let mine = verify(1000, &stored(1000, "jay", 120, 5)).unwrap();
        let theirs = verify(1001, &stored(1001, "jay", 50, 2)).unwrap();
        let amy = verify(1001, &stored(1001, "amy", 500, 1)).unwrap();
        assert!(
            verify(1000, &stored(1001, "amy", 500, 1)).is_none(),
            "別人的標籤搬到我的檔裡不算"
        );
        let b = Board::merge([theirs, amy, mine]);
        let rows: Vec<(&str, u32, u32)> = b
            .top()
            .iter()
            .map(|e| (e.name.as_str(), e.score, e.uid))
            .collect();
        assert_eq!(rows, vec![("amy", 500, 1001), ("jay", 120, 1000)]);
    }

    #[test]
    fn without_a_shared_dir_the_board_is_private_and_nothing_is_created_elsewhere() {
        let dir = scratch("private");
        let missing = dir.join("no-such-shared-dir");
        assert!(!dir_writable(&missing), "不存在的目錄不算共用");
        let store = Store::with_paths(None, dir.join("data").join("dino-scores.json"), me());
        assert!(!store.is_shared());
        store.record("me", 30).unwrap();
        assert!(dir.join("data").join("dino-scores.json").is_file());
        assert!(!missing.exists(), "共用目錄不該被建出來");
        assert_eq!(store.load().top()[0].name, "me");
    }

    #[test]
    fn a_users_file_is_capped_per_name_and_in_size() {
        let dir = scratch("cap");
        let store = Store::with_paths(Some(dir.clone()), dir.join(format!("{}.json", me())), me());
        for i in 0..(KEEP_PER_USER + 10) {
            store.record(&format!("n{i}"), 1000 + i as u32).unwrap();
        }
        assert_eq!(store.own_entries().len(), KEEP_PER_USER);
        assert!(
            std::fs::metadata(dir.join(format!("{}.json", me())))
                .unwrap()
                .len()
                < MAX_FILE
        );
    }
}
