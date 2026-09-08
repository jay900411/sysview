//! Process collector：走訪 `/proc/<pid>/`。
//!
//! 這是全系統最貴的一次取樣（幾百個行程 × 每個至少兩次 open/read），
//! 所以排程器會給它獨立且較長的取樣間隔。
//!
//! **行程隨時可能在 readdir 與 open 之間消失**，這是正常情況不是錯誤，
//! 全部靜默略過，絕不 panic。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::collectors::util;
use crate::metrics::model::Severity;

/// `/proc/<pid>/stat` 解析出來的欄位（只取用得到的）。
#[derive(Debug, Clone, PartialEq)]
pub struct ProcStat {
    pub comm: String,
    pub state: char,
    pub ppid: i32,
    pub utime: u64,
    pub stime: u64,
    pub nice: i64,
    pub num_threads: i64,
    pub starttime: u64,
    pub vsize: u64,
    pub rss_pages: i64,
}

/// 解析 `/proc/<pid>/stat`。
///
/// comm 欄位是**可執行檔名稱且可能包含空白與括號**，例如
/// `1234 (my app (v2)) S 1 ...`。所以必須從**最後一個** `)` 之後才開始
/// 用空白切欄位，否則所有欄位都會位移。這是這個檔案最常見的解析 bug。
pub fn parse_proc_stat(text: &str) -> Option<ProcStat> {
    let lp = text.find('(')?;
    let rp = text.rfind(')')?;
    if rp < lp {
        return None;
    }
    let comm = text[lp + 1..rp].to_owned();
    let rest = text.get(rp + 2..)?;
    let f: Vec<&str> = rest.split_ascii_whitespace().collect();
    // 從 state 開始算，索引 0 = state（整體第 3 欄）
    if f.len() < 22 {
        return None;
    }
    Some(ProcStat {
        state: f[0].chars().next().unwrap_or('?'),
        ppid: f[1].parse().ok()?,
        utime: f[11].parse().ok()?,
        stime: f[12].parse().ok()?,
        nice: f[16].parse().unwrap_or(0),
        num_threads: f[17].parse().unwrap_or(1),
        starttime: f[19].parse().unwrap_or(0),
        vsize: f[20].parse().unwrap_or(0),
        rss_pages: f[21].parse().unwrap_or(0),
        comm,
    })
}

/// 把 `/proc/<pid>/cmdline` 的 NUL 分隔內容轉成可讀字串。
/// 核心執行緒的 cmdline 是空的，這時退回 `[comm]`（與 ps 的慣例一致）。
pub fn format_cmdline(raw: &[u8], comm: &str) -> String {
    let s: String = String::from_utf8_lossy(raw)
        .chars()
        .map(|c| if c == '\0' { ' ' } else { c })
        .collect();
    let s = s.trim();
    if s.is_empty() {
        format!("[{comm}]")
    } else {
        s.to_owned()
    }
}

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: i32,
    pub ppid: i32,
    pub uid: u32,
    pub user: String,
    pub name: String,
    pub cmdline: String,
    pub state: char,
    pub cpu_percent: f64,
    pub rss: u64,
    pub vsize: u64,
    pub threads: i64,
    pub nice: i64,
    pub cpu_time_secs: f64,
    /// 核心啟動後多少 tick 時建立。與 pid 一起用可辨識 PID 重用。
    pub starttime: u64,
}

impl Process {
    pub fn state_label(&self) -> &'static str {
        match self.state {
            'R' => "執行",
            'S' => "睡眠",
            'D' => "IO等待",
            'Z' => "殭屍",
            'T' => "停止",
            't' => "追蹤",
            'I' => "閒置",
            'X' => "死亡",
            _ => "?",
        }
    }
    /// D（不可中斷 I/O）與 Z（殭屍）值得標出來。
    pub fn state_severity(&self) -> Severity {
        match self.state {
            'D' => Severity::Warning,
            'Z' => Severity::Critical,
            _ => Severity::Ok,
        }
    }
    pub fn mem_percent(&self, mem_total: u64) -> f64 {
        if mem_total == 0 {
            0.0
        } else {
            self.rss as f64 / mem_total as f64 * 100.0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Memory,
    Pid,
    Threads,
    User,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "rss",
            Self::Pid => "pid",
            Self::Threads => "thr",
            Self::User => "user",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Cpu => Self::Memory,
            Self::Memory => Self::Pid,
            Self::Pid => Self::Threads,
            Self::Threads => Self::User,
            Self::User => Self::Cpu,
        }
    }
}

#[derive(Debug, Default)]
pub struct ProcState {
    pub processes: Vec<Process>,
    pub total: usize,
    pub threads: i64,
    pub by_state: HashMap<char, usize>,
}

impl ProcState {
    /// 依排序鍵取出前 n 名。用 partial_sort 避免對幾百個行程做完整排序。
    pub fn top(&self, key: SortKey, n: usize, filter: &str) -> Vec<&Process> {
        let f = filter.to_ascii_lowercase();
        let mut v: Vec<&Process> = self
            .processes
            .iter()
            .filter(|p| {
                f.is_empty()
                    || p.name.to_ascii_lowercase().contains(&f)
                    || p.cmdline.to_ascii_lowercase().contains(&f)
                    || p.user.to_ascii_lowercase().contains(&f)
                    || p.pid.to_string() == f
            })
            .collect();
        sort_processes(&mut v, key);
        if n > 0 {
            v.truncate(n);
        }
        v
    }
}

pub fn sort_processes(v: &mut [&Process], key: SortKey) {
    match key {
        SortKey::Cpu => v.sort_by(|a, b| {
            b.cpu_percent
                .total_cmp(&a.cpu_percent)
                .then_with(|| a.pid.cmp(&b.pid))
        }),
        SortKey::Memory => v.sort_by(|a, b| b.rss.cmp(&a.rss).then_with(|| a.pid.cmp(&b.pid))),
        SortKey::Pid => v.sort_by_key(|p| p.pid),
        SortKey::Threads => {
            v.sort_by(|a, b| b.threads.cmp(&a.threads).then_with(|| a.pid.cmp(&b.pid)))
        }
        SortKey::User => v.sort_by(|a, b| a.user.cmp(&b.user).then_with(|| a.pid.cmp(&b.pid))),
    }
}

/// 真正去讀 `/proc` 的那個東西。它記得上一輪每個 PID 的 jiffies，
/// 所以每一輪的 CPU% 才算得出來 —— 這份狀態跟著它走到哪個執行緒都行。
struct Scanner {
    prev_jiffies: HashMap<i32, u64>,
    prev_instant: Option<Instant>,
    buf: String,
    raw: Vec<u8>,
}

impl Scanner {
    fn new() -> Self {
        Self {
            prev_jiffies: HashMap::with_capacity(1024),
            prev_instant: None,
            buf: String::with_capacity(4096),
            raw: Vec::with_capacity(4096),
        }
    }

    fn scan(&mut self, now: Instant, ncpu: usize) -> ProcState {
        let dt = self
            .prev_instant
            .map(|p| now.duration_since(p).as_secs_f64());
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return ProcState::default();
        };

        let mut out = Vec::with_capacity(self.prev_jiffies.len().max(256));
        let mut cur = HashMap::with_capacity(self.prev_jiffies.len().max(256));
        let mut threads = 0i64;
        let mut by_state: HashMap<char, usize> = HashMap::new();
        let clk = util::clock_ticks() as f64;
        let page = util::page_size();
        let max_cpu = 100.0 * ncpu.max(1) as f64;

        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(pid) = name.parse::<i32>() else {
                continue;
            };

            // 行程可能在這裡就已經結束了 —— 完全正常，靜默略過。
            let base = format!("/proc/{pid}");
            if util::read_into(format!("{base}/stat"), &mut self.buf).is_err() {
                continue;
            }
            let Some(st) = parse_proc_stat(&self.buf) else {
                continue;
            };

            let uid = std::fs::metadata(&base)
                .map(|m| std::os::unix::fs::MetadataExt::uid(&m))
                .unwrap_or(0);

            let jif = st.utime.saturating_add(st.stime);
            cur.insert(pid, jif);
            let cpu = match (self.prev_jiffies.get(&pid), dt) {
                (Some(&prev), Some(dt)) if dt > 1e-6 && jif >= prev => {
                    (((jif - prev) as f64 / clk) / dt * 100.0).clamp(0.0, max_cpu)
                }
                _ => 0.0,
            };

            self.raw.clear();
            let cmdline = match std::fs::read(format!("{base}/cmdline")) {
                Ok(b) => format_cmdline(&b, &st.comm),
                Err(_) => format!("[{}]", st.comm),
            };

            threads += st.num_threads;
            *by_state.entry(st.state).or_insert(0) += 1;
            out.push(Process {
                pid,
                ppid: st.ppid,
                uid,
                user: util::username(uid),
                name: st.comm,
                cmdline,
                state: st.state,
                cpu_percent: cpu,
                rss: (st.rss_pages.max(0) as u64).saturating_mul(page),
                vsize: st.vsize,
                threads: st.num_threads,
                nice: st.nice,
                cpu_time_secs: jif as f64 / clk,
                starttime: st.starttime,
            });
        }

        self.prev_jiffies = cur;
        self.prev_instant = Some(now);
        ProcState {
            total: out.len(),
            threads,
            by_state,
            processes: out,
        }
    }
}

/// 背景掃描的執行緒：一次請求、一份結果，順便回報那一輪掃了多久。
struct Worker {
    tx: std::sync::mpsc::Sender<(Instant, usize)>,
    rx: std::sync::mpsc::Receiver<(ProcState, Duration)>,
}

fn spawn_worker(mut scanner: Scanner) -> Option<Worker> {
    let (tx_req, rx_req) = std::sync::mpsc::channel::<(Instant, usize)>();
    let (tx_res, rx_res) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("sysview-proc".into())
        .spawn(move || {
            while let Ok((now, ncpu)) = rx_req.recv() {
                let t = Instant::now();
                let state = scanner.scan(now, ncpu);
                if tx_res.send((state, t.elapsed())).is_err() {
                    break;
                }
            }
        })
        .ok()?;
    Some(Worker {
        tx: tx_req,
        rx: rx_res,
    })
}

/// 行程清單。
///
/// 掃 `/proc` 是所有 collector 裡最貴的一件事（幾百個行程、每個開兩三個
/// 檔案，量到 4–5 ms，行程越多越久），而它本來跑在主執行緒上：每一秒有
/// 一格畫面要等它掃完才畫得出來 —— 玩彩蛋遊戲時就是那一下「卡」。
/// 所以掃描在背景執行緒上做，主執行緒只送請求、收結果，一次都不等。
///
/// 一次只會有一輪在掃（`pending`），掃完才收下一個請求；worker 掛了就
/// 重開一個。快照模式（`--json`）不需要背景執行緒，走同步的 [`Self::update`]。
pub struct ProcCollector {
    state: ProcState,
    /// 還沒交給 worker 之前，掃描器在這裡（同步模式一直都在這裡）
    scanner: Option<Scanner>,
    worker: Option<Worker>,
    pending: bool,
    last_cost: Duration,
}

impl ProcCollector {
    pub fn new() -> Self {
        Self {
            state: ProcState::default(),
            scanner: Some(Scanner::new()),
            worker: None,
            pending: false,
            last_cost: Duration::ZERO,
        }
    }
    pub fn state(&self) -> &ProcState {
        &self.state
    }

    /// 同步掃一輪，掃完才回來。快照模式與測試用；互動模式走
    /// [`Self::request`] + [`Self::poll`]。
    pub fn update(&mut self, now: Instant, ncpu: usize) {
        let t = Instant::now();
        if let Some(sc) = self.scanner.as_mut() {
            self.state = sc.scan(now, ncpu);
            self.last_cost = t.elapsed();
            return;
        }
        // 掃描器已經在 worker 那邊：請它掃，等結果
        self.request(now, ncpu);
        if let Some(w) = &self.worker {
            if let Ok((state, cost)) = w.rx.recv() {
                self.state = state;
                self.last_cost = cost;
            }
        }
        self.pending = false;
    }

    /// 請 worker 掃一輪，**不等**。已經有一輪在掃就不重複送。
    pub fn request(&mut self, now: Instant, ncpu: usize) {
        if self.pending {
            return;
        }
        if self.worker.is_none() {
            let sc = self.scanner.take().unwrap_or_else(Scanner::new);
            match spawn_worker(sc) {
                Some(w) => self.worker = Some(w),
                None => {
                    // 開不出執行緒（極少見）：退回同步掃描
                    self.scanner = Some(Scanner::new());
                    self.update(now, ncpu);
                    return;
                }
            }
        }
        let sent = self
            .worker
            .as_ref()
            .is_some_and(|w| w.tx.send((now, ncpu)).is_ok());
        if !sent {
            // worker 死了：重開一個再送一次
            self.worker = spawn_worker(Scanner::new());
            if let Some(w) = &self.worker {
                let _ = w.tx.send((now, ncpu));
            }
        }
        self.pending = self.worker.is_some();
    }

    /// 收 worker 的結果。有新的一輪就換上並回 `true`。
    pub fn poll(&mut self) -> bool {
        let Some(w) = &self.worker else {
            return false;
        };
        let mut got = false;
        loop {
            match w.rx.try_recv() {
                Ok((state, cost)) => {
                    self.state = state;
                    self.last_cost = cost;
                    self.pending = false;
                    got = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // 下一次 request 會重開
                    self.pending = false;
                    break;
                }
            }
        }
        got
    }

    /// 有一輪還在背景掃。主迴圈用它決定要不要早點醒來收結果。
    pub fn is_pending(&self) -> bool {
        self.pending
    }

    /// 上一輪掃了多久（在 worker 上量的）。
    pub fn last_cost(&self) -> Duration {
        self.last_cost
    }
}

impl Default for ProcCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 真實的 /proc/<pid>/stat 樣本（欄位已截到需要的長度）
    const SAMPLE: &str = "1322 (syncsvc) S 1 1322 1322 0 -1 4194560 12345 0 0 0 \
        4500 1200 0 0 20 0 4 0 987654 367001600 5632 18446744073709551615 1 1 0 0 0 0 0 0 0";

    #[test]
    fn parses_normal_stat() {
        let s = parse_proc_stat(SAMPLE).unwrap();
        assert_eq!(s.comm, "syncsvc");
        assert_eq!(s.state, 'S');
        assert_eq!(s.ppid, 1);
        assert_eq!(s.utime, 4500);
        assert_eq!(s.stime, 1200);
        assert_eq!(s.num_threads, 4);
        assert_eq!(s.rss_pages, 5632);
    }

    #[test]
    fn handles_comm_containing_spaces_and_parens() {
        // 這是最經典的 /proc 解析 bug：用第一個 ')' 切會讓所有欄位位移
        let text = "42 (my app (v2) [x]) R 1 42 42 0 -1 0 0 0 0 0 \
            10 20 0 0 20 0 7 0 123 4096 99 0 0 0 0 0 0 0 0";
        let s = parse_proc_stat(text).unwrap();
        assert_eq!(s.comm, "my app (v2) [x]");
        assert_eq!(s.state, 'R');
        assert_eq!(s.utime, 10);
        assert_eq!(s.stime, 20);
        assert_eq!(s.num_threads, 7);
    }

    #[test]
    fn handles_kernel_thread_bracket_names() {
        let text = "2 (kthreadd) S 0 0 0 0 -1 2129984 0 0 0 0 \
            0 5 0 0 20 0 1 0 3 0 0 0 0 0 0 0 0 0";
        let s = parse_proc_stat(text).unwrap();
        assert_eq!(s.comm, "kthreadd");
        assert_eq!(s.rss_pages, 0);
    }

    #[test]
    fn rejects_malformed_and_truncated() {
        assert!(parse_proc_stat("").is_none());
        assert!(parse_proc_stat("no parens here").is_none());
        assert!(parse_proc_stat("1 (x) S 1 2 3").is_none(), "欄位不足要拒絕");
        assert!(parse_proc_stat("1 ) x ( S").is_none(), "括號順序顛倒要拒絕");
    }

    #[test]
    fn cmdline_nul_separated_becomes_spaces() {
        let raw = b"rsync\0-aHAX\0--numeric-ids\0/src/\0/dst/\0";
        assert_eq!(
            format_cmdline(raw, "rsync"),
            "rsync -aHAX --numeric-ids /src/ /dst/"
        );
    }

    #[test]
    fn empty_cmdline_falls_back_to_bracketed_comm() {
        assert_eq!(format_cmdline(b"", "kworker/0:1"), "[kworker/0:1]");
        assert_eq!(format_cmdline(b"\0\0", "kswapd0"), "[kswapd0]");
    }

    #[test]
    fn cmdline_survives_invalid_utf8() {
        let raw = b"prog\0\xff\xfe\0arg";
        let s = format_cmdline(raw, "prog");
        assert!(
            s.starts_with("prog"),
            "非 UTF-8 位元組不可造成 panic，得到 {s:?}"
        );
    }

    fn mkproc(pid: i32, cpu: f64, rss: u64, thr: i64, user: &str) -> Process {
        Process {
            pid,
            ppid: 1,
            uid: 1000,
            user: user.into(),
            name: format!("p{pid}"),
            cmdline: format!("/bin/p{pid}"),
            state: 'S',
            cpu_percent: cpu,
            rss,
            vsize: 0,
            threads: thr,
            nice: 0,
            cpu_time_secs: 0.0,
            starttime: 0,
        }
    }

    #[test]
    fn sorts_by_each_key() {
        let ps = [
            mkproc(3, 5.0, 300, 1, "carol"),
            mkproc(1, 90.0, 100, 9, "alice"),
            mkproc(2, 50.0, 900, 4, "bob"),
        ];
        let mut v: Vec<&Process> = ps.iter().collect();
        sort_processes(&mut v, SortKey::Cpu);
        assert_eq!(v[0].pid, 1);
        sort_processes(&mut v, SortKey::Memory);
        assert_eq!(v[0].pid, 2);
        sort_processes(&mut v, SortKey::Pid);
        assert_eq!(v[0].pid, 1);
        sort_processes(&mut v, SortKey::Threads);
        assert_eq!(v[0].pid, 1);
        sort_processes(&mut v, SortKey::User);
        assert_eq!(v[0].user, "alice");
    }

    #[test]
    fn filter_matches_name_user_and_pid() {
        let st = ProcState {
            processes: vec![mkproc(11, 1.0, 1, 1, "alice"), mkproc(22, 2.0, 2, 1, "bob")],
            ..Default::default()
        };
        assert_eq!(st.top(SortKey::Cpu, 0, "alice").len(), 1);
        assert_eq!(st.top(SortKey::Cpu, 0, "p22").len(), 1);
        assert_eq!(st.top(SortKey::Cpu, 0, "22").len(), 1, "整數應能比對 PID");
        assert_eq!(st.top(SortKey::Cpu, 0, "").len(), 2);
        assert_eq!(st.top(SortKey::Cpu, 0, "nomatch").len(), 0);
    }

    #[test]
    fn sort_key_cycles_through_all() {
        let mut k = SortKey::Cpu;
        for _ in 0..5 {
            k = k.next();
        }
        assert_eq!(k, SortKey::Cpu, "循環一輪應回到起點");
    }
}
