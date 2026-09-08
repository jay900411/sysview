//! Storage collector：`/proc/mounts` + `statvfs(2)` + `/proc/diskstats`。

use std::collections::HashMap;
use std::ffi::CString;
use std::time::Instant;

use crate::collectors::thermal;
use crate::collectors::util::{self, Counter};
use crate::error::Unavailable;
use crate::metrics::model::{Reading, Series, Severity, Unit};

/// 這些是虛擬檔案系統，不佔實體空間，列出來只會洗版。
const PSEUDO_FS: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "squashfs",
    "overlay",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "devpts",
    "efivarfs",
    "autofs",
    "mqueue",
    "hugetlbfs",
    "debugfs",
    "tracefs",
    "pstore",
    "bpf",
    "configfs",
    "fusectl",
    "securityfs",
    "ramfs",
    "binfmt_misc",
    "nsfs",
    "fuse.gvfsd-fuse",
    "fuse.portal",
    "rpc_pipefs",
    "tmpfs",
    "selinuxfs",
];

/// `/proc/mounts` 的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
}

/// 解析 `/proc/mounts`。
///
/// 掛載點裡的空白會被核心編碼成 `\040`，tab 是 `\011`，換行 `\012`，反斜線 `\134`。
/// 不解碼的話含空白的路徑會被切錯。
pub fn parse_mounts(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split_ascii_whitespace();
            let device = unescape_mount(it.next()?);
            let mount_point = unescape_mount(it.next()?);
            let fs_type = it.next()?.to_owned();
            Some(MountEntry {
                device,
                mount_point,
                fs_type,
            })
        })
        .collect()
}

fn unescape_mount(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_owned();
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() {
            let oct = &s[i + 1..i + 4];
            if let Ok(v) = u8::from_str_radix(oct, 8) {
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// 該不該把這個掛載點列給使用者看。
pub fn is_real_filesystem(m: &MountEntry) -> bool {
    m.device.starts_with("/dev/") && !PSEUDO_FS.contains(&m.fs_type.as_str())
}

/// 一個掛載點的容量資訊。
#[derive(Debug, Clone)]
pub struct Mount {
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub inodes_total: u64,
    pub inodes_used: u64,
    pub usage: Reading,
    pub inode_usage: Reading,
}

impl Mount {
    pub fn severity(&self) -> Severity {
        match self.usage.get() {
            Some(p) if p >= 95.0 => Severity::Critical,
            Some(p) if p >= 85.0 => Severity::Warning,
            Some(p) if p >= 70.0 => Severity::Notice,
            Some(_) => Severity::Ok,
            None => Severity::Unknown,
        }
    }
}

/// 用 `statvfs(2)` 取容量。回 `None` 代表掛載點在讀取瞬間消失或無權限。
pub fn statvfs(path: &str) -> Option<libc::statvfs> {
    let c = CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: c 是合法的 NUL 結尾字串，st 是正確大小的可寫緩衝區。
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc == 0 {
        Some(st)
    } else {
        None
    }
}

/// 從 statvfs 算出容量數字。
///
/// **使用率採用 df 的定義**：`used / (used + available)`。
/// ext4 預設保留 5% 給 root，df 的 Use% 把那塊排除在外，
/// 用 `used / total` 會比 df 少報好幾個百分點。
pub fn mount_from_statvfs(e: &MountEntry, st: &libc::statvfs) -> Option<Mount> {
    let frsize = if st.f_frsize > 0 {
        st.f_frsize
    } else {
        st.f_bsize
    };
    if st.f_blocks == 0 || frsize == 0 {
        return None;
    }
    let total = st.f_blocks * frsize;
    let available = st.f_bavail * frsize;
    let used = total.saturating_sub(st.f_bfree * frsize);
    let denom = used + available;
    let usage = if denom == 0 {
        Reading::unavailable(
            Unit::Percent,
            Unavailable::Malformed("empty filesystem".into()),
        )
    } else {
        Reading::exact(used as f64 / denom as f64 * 100.0, Unit::Percent).with_id("disk.usage")
    };
    let inodes_total = st.f_files;
    let inodes_used = inodes_total.saturating_sub(st.f_ffree);
    let inode_usage = if inodes_total == 0 {
        // XFS/Btrfs 動態配置 inode，這個數字沒有意義
        Reading::unavailable(Unit::Percent, Unavailable::Unsupported).with_id("disk.inodes")
    } else {
        Reading::exact(
            inodes_used as f64 / inodes_total as f64 * 100.0,
            Unit::Percent,
        )
        .with_id("disk.inodes")
    };
    Some(Mount {
        device: e.device.clone(),
        mount_point: e.mount_point.clone(),
        fs_type: e.fs_type.clone(),
        total,
        used,
        available,
        inodes_total,
        inodes_used,
        usage,
        inode_usage,
    })
}

/// bind mount（snap、docker overlay 的 lowerdir 等）會讓同一個檔案系統
/// 出現很多次，統計數字完全相同。只留第一個，否則清單會被洗版。
pub fn dedupe_bind_mounts(mut mounts: Vec<Mount>) -> Vec<Mount> {
    let mut seen = std::collections::HashSet::new();
    mounts.retain(|m| seen.insert((m.device.clone(), m.total, m.used, m.inodes_total)));
    mounts.sort_by(|a, b| {
        b.total
            .cmp(&a.total)
            .then_with(|| a.mount_point.cmp(&b.mount_point))
    });
    mounts
}

/// `/proc/diskstats` 的一行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStat {
    pub reads_completed: u64,
    pub sectors_read: u64,
    pub writes_completed: u64,
    pub sectors_written: u64,
    /// 有 I/O 進行中的累計毫秒數（第 13 欄）
    pub io_ticks_ms: u64,
}

/// 解析 `/proc/diskstats` 一行，回傳 (裝置名, 統計)。
pub fn parse_diskstat_line(line: &str) -> Option<(String, DiskStat)> {
    let f: Vec<&str> = line.split_ascii_whitespace().collect();
    if f.len() < 14 {
        return None;
    }
    let p = |i: usize| f[i].parse::<u64>().ok();
    Some((
        f[2].to_owned(),
        DiskStat {
            reads_completed: p(3)?,
            sectors_read: p(5)?,
            writes_completed: p(7)?,
            sectors_written: p(9)?,
            io_ticks_ms: p(12)?,
        },
    ))
}

/// 是不是分割區（而非整顆磁碟）。只統計整顆碟，否則吞吐量會被加倍計算。
pub fn is_partition(name: &str) -> bool {
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        // nvme0n1p2 / mmcblk0p1 —— 有 'p' 後接數字才是分割區
        return name.rsplit_once('p').is_some_and(|(head, tail)| {
            !tail.is_empty()
                && tail.bytes().all(|c| c.is_ascii_digit())
                && head.bytes().any(|c| c.is_ascii_digit())
        });
    }
    // sda1 / vdb3 / hdc2
    (name.starts_with("sd") || name.starts_with("vd") || name.starts_with("hd"))
        && name.chars().last().is_some_and(|c| c.is_ascii_digit())
}

/// 這個裝置該不該顯示。loop 是 snap，dm- 是 LVM/LUKS 映射，ram/zram 是虛擬的。
pub fn is_reportable_device(name: &str) -> bool {
    !(name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("zram")
        || name.starts_with("dm-")
        || name.starts_with("sr"))
        && !is_partition(name)
}

#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    pub model: String,
    pub rotational: Option<bool>,
    pub size: Option<u64>,
    pub read_rate: Reading,
    pub write_rate: Reading,
    pub iops: Reading,
    pub util: Reading,
    pub temp: Reading,
    pub read_history: Series,
    pub write_history: Series,
    read_ctr: Counter,
    write_ctr: Counter,
    io_ctr: Counter,
    ticks_ctr: Counter,
}

impl Device {
    fn new(name: String, history_len: usize) -> Self {
        let base = format!("/sys/block/{name}");
        let model = util::read_trimmed(format!("{base}/device/model"))
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| name.clone());
        let rotational = util::read_i64_opt(format!("{base}/queue/rotational")).map(|v| v == 1);
        let size = util::read_i64_opt(format!("{base}/size")).map(|s| s as u64 * 512);
        Self {
            name,
            model,
            rotational,
            size,
            read_rate: Reading::unavailable(Unit::BytesPerSec, Unavailable::Pending),
            write_rate: Reading::unavailable(Unit::BytesPerSec, Unavailable::Pending),
            iops: Reading::unavailable(Unit::CountPerSec, Unavailable::Pending),
            util: Reading::unavailable(Unit::Percent, Unavailable::Pending),
            temp: Reading::unsupported(Unit::Celsius),
            read_history: Series::new(history_len),
            write_history: Series::new(history_len),
            read_ctr: Counter::new(),
            write_ctr: Counter::new(),
            io_ctr: Counter::new(),
            ticks_ctr: Counter::new(),
        }
    }
    pub fn kind(&self) -> &'static str {
        match self.rotational {
            Some(true) => "HDD",
            Some(false) => "SSD",
            None => "?",
        }
    }
}

#[derive(Default)]
pub struct DiskState {
    pub mounts: Vec<Mount>,
    pub devices: Vec<Device>,
    pub total_read: Reading,
    pub total_write: Reading,
    pub read_history: Series,
    pub write_history: Series,
}

pub struct DiskCollector {
    state: DiskState,
    index: HashMap<String, usize>,
    buf: String,
    history_len: usize,
    last_mount_scan: Option<Instant>,
    chips: Vec<thermal::Chip>,
}

impl DiskCollector {
    pub fn new(history_len: usize) -> Self {
        Self {
            state: DiskState {
                read_history: Series::new(history_len),
                write_history: Series::new(history_len),
                ..Default::default()
            },
            index: HashMap::new(),
            buf: String::with_capacity(16384),
            history_len,
            last_mount_scan: None,
            chips: thermal::scan(),
        }
    }
    pub fn state(&self) -> &DiskState {
        &self.state
    }

    /// 掛載點掃描比 diskstats 貴（每個掛載點一次 statvfs syscall，
    /// 而且對掛掉的 NFS 會阻塞），所以獨立節流成 5 秒一次。
    pub fn update(&mut self, now: Instant) {
        let need_mounts = self
            .last_mount_scan
            .is_none_or(|t| now.duration_since(t).as_secs_f64() >= 5.0);
        if need_mounts {
            self.scan_mounts();
            self.last_mount_scan = Some(now);
        }
        self.update_io(now);
    }

    fn scan_mounts(&mut self) {
        let Ok(text) = util::read_string("/proc/mounts") else {
            return;
        };
        let mounts: Vec<Mount> = parse_mounts(&text)
            .into_iter()
            .filter(is_real_filesystem)
            .filter_map(|e| {
                let st = statvfs(&e.mount_point)?;
                mount_from_statvfs(&e, &st)
            })
            .collect();
        self.state.mounts = dedupe_bind_mounts(mounts);

        // NVMe 溫度：hwmon 的 device symlink 指回區塊裝置名
        let temps = thermal::nvme_temps(&self.chips);
        for d in &mut self.state.devices {
            d.temp = match temps.iter().find(|(n, _)| *n == d.name) {
                Some((_, t)) => Reading::exact(*t, Unit::Celsius),
                None => Reading::unsupported(Unit::Celsius),
            };
        }
    }

    fn update_io(&mut self, now: Instant) {
        if util::read_into("/proc/diskstats", &mut self.buf).is_err() {
            return;
        }
        let text = std::mem::take(&mut self.buf);
        let mut total_r = 0.0;
        let mut total_w = 0.0;
        let mut any = false;

        for line in text.lines() {
            let Some((name, s)) = parse_diskstat_line(line) else {
                continue;
            };
            if !is_reportable_device(&name) {
                continue;
            }
            let idx = match self.index.get(&name) {
                Some(&i) => i,
                None => {
                    let i = self.state.devices.len();
                    self.state
                        .devices
                        .push(Device::new(name.clone(), self.history_len));
                    self.index.insert(name, i);
                    i
                }
            };
            let d = &mut self.state.devices[idx];
            if let Some(r) = d.read_ctr.update(s.sectors_read.saturating_mul(512), now) {
                d.read_rate = Reading::exact(r, Unit::BytesPerSec).with_id("disk.throughput");
                d.read_history.push(r);
                total_r += r;
                any = true;
            }
            if let Some(w) = d
                .write_ctr
                .update(s.sectors_written.saturating_mul(512), now)
            {
                d.write_rate = Reading::exact(w, Unit::BytesPerSec).with_id("disk.throughput");
                d.write_history.push(w);
                total_w += w;
            }
            if let Some(io) = d.io_ctr.update(s.reads_completed + s.writes_completed, now) {
                d.iops = Reading::exact(io, Unit::CountPerSec).with_id("disk.iops");
            }
            if let Some(t) = d.ticks_ctr.update(s.io_ticks_ms, now) {
                // io_ticks 是毫秒/秒 → ÷10 得百分比
                d.util = Reading::exact((t / 10.0).clamp(0.0, 100.0), Unit::Percent)
                    .with_id("disk.util");
            }
        }
        self.buf = text;
        if any {
            self.state.total_read =
                Reading::exact(total_r, Unit::BytesPerSec).with_id("disk.throughput");
            self.state.total_write =
                Reading::exact(total_w, Unit::BytesPerSec).with_id("disk.throughput");
            self.state.read_history.push(total_r);
            self.state.write_history.push(total_w);
        }
        self.state
            .devices
            .sort_by(|a, b| b.read_rate.or_zero().total_cmp(&a.read_rate.or_zero()));
        self.index = self
            .state
            .devices
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name.clone(), i))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mounts() {
        let m = parse_mounts(
            "/dev/nvme0n1p2 / ext4 rw,relatime 0 0\n\
             proc /proc proc rw 0 0\n\
             /dev/sda1 /mnt/data ext4 rw 0 0\n",
        );
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].mount_point, "/");
        assert!(is_real_filesystem(&m[0]));
        assert!(!is_real_filesystem(&m[1]), "proc 不是實體檔案系統");
        assert!(is_real_filesystem(&m[2]));
    }

    #[test]
    fn decodes_octal_escapes_in_paths() {
        let m = parse_mounts("/dev/sdb1 /mnt/my\\040disk ext4 rw 0 0\n");
        assert_eq!(
            m[0].mount_point, "/mnt/my disk",
            "含空白的掛載點必須解碼 \\040"
        );
    }

    #[test]
    fn ignores_short_and_empty_lines() {
        assert!(parse_mounts("").is_empty());
        assert!(parse_mounts("only two\n").is_empty());
    }

    #[test]
    fn identifies_partitions_not_whole_disks() {
        assert!(is_partition("sda1"));
        assert!(is_partition("nvme0n1p2"));
        assert!(is_partition("mmcblk0p1"));
        assert!(!is_partition("sda"));
        assert!(
            !is_partition("nvme0n1"),
            "nvme0n1 是整顆碟，結尾數字不代表分割區"
        );
        assert!(!is_partition("mmcblk0"));
    }

    #[test]
    fn filters_virtual_devices() {
        assert!(is_reportable_device("sda"));
        assert!(is_reportable_device("nvme0n1"));
        assert!(!is_reportable_device("loop12"), "loop 是 snap，不該列");
        assert!(!is_reportable_device("dm-0"));
        assert!(!is_reportable_device("zram0"));
        assert!(!is_reportable_device("sda1"), "分割區會與整顆碟重複計算");
    }

    #[test]
    fn parses_diskstats() {
        let line = "259 0 nvme0n1 1000 0 20000 500 2000 0 40000 900 0 1234 1400";
        let (name, s) = parse_diskstat_line(line).unwrap();
        assert_eq!(name, "nvme0n1");
        assert_eq!(s.reads_completed, 1000);
        assert_eq!(s.sectors_read, 20000);
        assert_eq!(s.sectors_written, 40000);
        assert_eq!(s.io_ticks_ms, 1234);
    }

    #[test]
    fn rejects_truncated_diskstats() {
        assert!(parse_diskstat_line("259 0 nvme0n1 1 2 3").is_none());
        assert!(parse_diskstat_line("").is_none());
    }

    #[test]
    fn usage_matches_df_semantics_not_used_over_total() {
        // 模擬 ext4 保留 5%：total 1000 區塊、free 200、可用 150（50 保留給 root）
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        st.f_frsize = 1024;
        st.f_bsize = 1024;
        st.f_blocks = 1000;
        st.f_bfree = 200;
        st.f_bavail = 150;
        st.f_files = 100;
        st.f_ffree = 60;
        let e = MountEntry {
            device: "/dev/x".into(),
            mount_point: "/".into(),
            fs_type: "ext4".into(),
        };
        let m = mount_from_statvfs(&e, &st).unwrap();
        assert_eq!(m.used, 800 * 1024);
        assert_eq!(m.available, 150 * 1024);
        // df: 800/(800+150) = 84.2%，而不是 800/1000 = 80%
        let p = m.usage.get().unwrap();
        assert!((p - 84.21).abs() < 0.1, "應符合 df 的 Use% 定義，得到 {p}");
        assert_eq!(m.inode_usage.get(), Some(40.0));
    }

    #[test]
    fn zero_block_filesystem_is_rejected() {
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        st.f_frsize = 1024;
        let e = MountEntry {
            device: "/dev/x".into(),
            mount_point: "/x".into(),
            fs_type: "ext4".into(),
        };
        assert!(mount_from_statvfs(&e, &st).is_none());
    }

    #[test]
    fn dynamic_inode_filesystems_report_unsupported() {
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        st.f_frsize = 4096;
        st.f_blocks = 100;
        st.f_bfree = 50;
        st.f_bavail = 50;
        st.f_files = 0; // XFS/Btrfs
        let e = MountEntry {
            device: "/dev/x".into(),
            mount_point: "/x".into(),
            fs_type: "btrfs".into(),
        };
        let m = mount_from_statvfs(&e, &st).unwrap();
        assert!(!m.inode_usage.quality.is_available());
    }

    fn mk(dev: &str, mp: &str, total: u64, used: u64) -> Mount {
        Mount {
            device: dev.into(),
            mount_point: mp.into(),
            fs_type: "ext4".into(),
            total,
            used,
            available: total - used,
            inodes_total: 10,
            inodes_used: 1,
            usage: Reading::exact(50.0, Unit::Percent),
            inode_usage: Reading::exact(10.0, Unit::Percent),
        }
    }

    #[test]
    fn dedupes_snap_style_bind_mounts() {
        let v = vec![
            mk("/dev/nvme0n1p2", "/", 900, 700),
            mk(
                "/dev/nvme0n1p2",
                "/var/snap/firefox/common/host-hunspell",
                900,
                700,
            ),
            mk("/dev/sda1", "/mnt/data", 2000, 10),
        ];
        let out = dedupe_bind_mounts(v);
        assert_eq!(out.len(), 2, "統計完全相同的 bind mount 應去重");
        assert_eq!(out[0].mount_point, "/mnt/data", "應依容量由大到小排序");
    }

    #[test]
    fn keeps_distinct_filesystems_on_same_device_name() {
        let v = vec![
            mk("/dev/sda1", "/a", 100, 10),
            mk("/dev/sda1", "/b", 100, 20),
        ];
        assert_eq!(dedupe_bind_mounts(v).len(), 2, "used 不同就不是 bind mount");
    }
}
