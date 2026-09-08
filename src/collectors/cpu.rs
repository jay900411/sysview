//! CPU collector：`/proc/stat`、`/proc/loadavg`、cpufreq、hwmon。

use std::time::Instant;

use crate::collectors::util::{self, Counter, CpuDelta};
use crate::error::Unavailable;
use crate::metrics::model::{Quality, Reading, Series, Unit};

/// `/proc/stat` 其中一行 `cpu*` 解析後的結果。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    /// 全部欄位加總。注意 guest / guest_nice 已包含在 user / nice 裡，
    /// 重複加會讓分母偏大、使用率偏低，所以只取前 8 欄。
    pub fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }
    /// idle 的定義包含 iowait —— iowait 時 CPU 確實是閒著的。
    pub fn idle_all(&self) -> u64 {
        self.idle + self.iowait
    }
}

/// 解析 `/proc/stat` 的一行 `cpu` / `cpuN`。
///
/// 欄位不足時盡量填（舊核心沒有 steal），完全不合法則回 `None`。
pub fn parse_cpu_line(line: &str) -> Option<(Option<usize>, CpuTimes)> {
    let mut it = line.split_ascii_whitespace();
    let tag = it.next()?;
    let rest = tag.strip_prefix("cpu")?;
    let index = if rest.is_empty() {
        None
    } else {
        Some(rest.parse::<usize>().ok()?)
    };
    let v: Vec<u64> = it.filter_map(|f| f.parse::<u64>().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let g = |i: usize| v.get(i).copied().unwrap_or(0);
    Some((
        index,
        CpuTimes {
            user: g(0),
            nice: g(1),
            system: g(2),
            idle: g(3),
            iowait: g(4),
            irq: g(5),
            softirq: g(6),
            steal: g(7),
        },
    ))
}

/// `/proc/stat` 裡非 `cpu*` 的全域計數器。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StatCounters {
    pub ctxt: Option<u64>,
    pub intr: Option<u64>,
    pub forks: Option<u64>,
    pub procs_running: Option<u64>,
    pub procs_blocked: Option<u64>,
}

pub fn parse_stat_counters(text: &str) -> StatCounters {
    let mut c = StatCounters::default();
    for line in text.lines() {
        let mut it = line.split_ascii_whitespace();
        let (Some(key), Some(val)) = (it.next(), it.next()) else {
            continue;
        };
        let v = val.parse::<u64>().ok();
        match key {
            "ctxt" => c.ctxt = v,
            "intr" => c.intr = v,
            "processes" => c.forks = v,
            "procs_running" => c.procs_running = v,
            "procs_blocked" => c.procs_blocked = v,
            _ => {}
        }
    }
    c
}

/// 解析 `/proc/loadavg` 前三個數字。
pub fn parse_loadavg(text: &str) -> Option<[f64; 3]> {
    let mut it = text.split_ascii_whitespace();
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    Some([a, b, c])
}

/// 一顆邏輯處理器在拓樸上的位置。
///
/// 溫度感測器是掛在**實體核心**上的，不是邏輯處理器 —— 開了超執行緒之後
/// 兩顆邏輯處理器共用一個感測器。要把 coretemp 的讀值對回畫面上的 cpuN，
/// 就得先知道 cpuN 坐落在哪個封裝的哪顆實體核心上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoreId {
    pub package: u32,
    pub core: u32,
}

/// 讀出每顆邏輯處理器的拓樸位置。
///
/// 優先用 sysfs 的 topology 節點；沒有的話（容器、老核心、某些 ARM 平台）
/// 回退到 `/proc/cpuinfo`。兩個都拿不到就回空的 —— 寧可不顯示溫度，
/// 也不要把某顆核心的溫度標到別顆頭上。
fn read_topology(logical: usize) -> Vec<CoreId> {
    let mut v = Vec::with_capacity(logical);
    for i in 0..logical {
        let base = format!("/sys/devices/system/cpu/cpu{i}/topology");
        let core = util::read_i64(format!("{base}/core_id")).ok();
        let pkg = util::read_i64(format!("{base}/physical_package_id")).ok();
        match (core, pkg) {
            (Some(c), Some(p)) => v.push(CoreId {
                package: p.max(0) as u32,
                core: c.max(0) as u32,
            }),
            _ => {
                v.clear();
                return topology_from_cpuinfo(logical);
            }
        }
    }
    v
}

/// `/proc/cpuinfo` 的回退路徑。x86 的 cpuinfo 有 `physical id` 與 `core id`，
/// 其他架構通常沒有，那就回空的。
fn topology_from_cpuinfo(logical: usize) -> Vec<CoreId> {
    let Ok(text) = util::read_string("/proc/cpuinfo") else {
        return Vec::new();
    };
    let mut v = Vec::new();
    let (mut pkg, mut core) = (None, None);
    for line in text.lines() {
        let Some((k, val)) = line.split_once(':') else {
            // 空行代表一顆處理器的區塊結束
            if let (Some(p), Some(c)) = (pkg.take(), core.take()) {
                v.push(CoreId {
                    package: p,
                    core: c,
                });
            }
            continue;
        };
        let val = val.trim();
        match k.trim() {
            "physical id" => pkg = val.parse().ok(),
            "core id" => core = val.parse().ok(),
            _ => {}
        }
    }
    if let (Some(p), Some(c)) = (pkg, core) {
        v.push(CoreId {
            package: p,
            core: c,
        });
    }
    if v.len() == logical {
        v
    } else {
        Vec::new()
    }
}

/// 單一邏輯核心的狀態。
#[derive(Debug, Clone)]
pub struct Core {
    pub index: usize,
    pub usage: Reading,
    pub freq_mhz: Reading,
    pub history: Series,
    delta: CpuDelta,
}

/// CPU 的完整快照。
#[derive(Debug, Clone)]
pub struct CpuState {
    pub model: String,
    pub logical: usize,
    pub physical: Option<usize>,
    pub usage: Reading,
    pub history: Series,
    pub cores: Vec<Core>,
    pub load: [f64; 3],
    pub iowait: Reading,
    pub steal: Reading,
    pub temp: Reading,
    /// 每個**實體核心**的溫度。key 是實體核心 id，不是邏輯處理器編號。
    ///
    /// coretemp 的 label 寫的是 `Core <core_id>`，那是實體核心編號。
    /// 在 i7-13700 上 P-core 的 core_id 是 0/4/8/12/16/20/24/28，
    /// E-core 是 32–39 —— 直接拿來當 cpuN 會只對上四分之一的核心。
    /// 要對回邏輯處理器請用 [`CpuState::core_temp`]。
    pub core_temps: Vec<(CoreId, f64)>,
    /// 邏輯處理器 → 它所在的實體核心。索引就是邏輯處理器編號。
    pub topology: Vec<CoreId>,
    pub ctxt_rate: Reading,
    pub intr_rate: Reading,
    pub fork_rate: Reading,
    pub running: Reading,
    pub blocked: Reading,
    pub uptime_secs: f64,
}

impl Default for CpuState {
    fn default() -> Self {
        Self {
            model: "unknown".into(),
            logical: 0,
            physical: None,
            usage: Reading::unavailable(Unit::Percent, Unavailable::Pending),
            history: Series::default(),
            cores: Vec::new(),
            load: [0.0; 3],
            iowait: Reading::unavailable(Unit::Percent, Unavailable::Pending),
            steal: Reading::unavailable(Unit::Percent, Unavailable::Pending),
            temp: Reading::unsupported(Unit::Celsius),
            core_temps: Vec::new(),
            topology: Vec::new(),
            ctxt_rate: Reading::unavailable(Unit::CountPerSec, Unavailable::Pending),
            intr_rate: Reading::unavailable(Unit::CountPerSec, Unavailable::Pending),
            fork_rate: Reading::unavailable(Unit::CountPerSec, Unavailable::Pending),
            running: Reading::unavailable(Unit::Count, Unavailable::Pending),
            blocked: Reading::unavailable(Unit::Count, Unavailable::Pending),
            uptime_secs: 0.0,
        }
    }
}

pub struct CpuCollector {
    state: CpuState,
    total_delta: CpuDelta,
    prev_total: CpuTimes,
    primed: bool,
    ctxt: Counter,
    intr: Counter,
    forks: Counter,
    /// 所有 CPU 溫度 hwmon 裝置。雙路機器一個封裝一個，不能只取第一個。
    hwmon: Vec<std::path::PathBuf>,
    buf: String,
    history_len: usize,
}

impl CpuCollector {
    pub fn new(history_len: usize) -> Self {
        let mut s = Self {
            state: CpuState::default(),
            total_delta: CpuDelta::default(),
            prev_total: CpuTimes::default(),
            primed: false,
            ctxt: Counter::new(),
            intr: Counter::new(),
            forks: Counter::new(),
            hwmon: find_cpu_hwmons(),
            buf: String::with_capacity(8192),
            history_len,
        };
        s.state.history = Series::new(history_len);
        s.read_cpuinfo();
        s
    }

    pub fn state(&self) -> &CpuState {
        &self.state
    }

    fn read_cpuinfo(&mut self) {
        let Ok(text) = util::read_string("/proc/cpuinfo") else {
            return;
        };
        let mut cores = std::collections::HashSet::new();
        for line in text.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            match k.trim() {
                "model name" if self.state.model == "unknown" => {
                    self.state.model = v.trim().to_owned();
                }
                "core id" => {
                    if let Ok(id) = v.trim().parse::<usize>() {
                        cores.insert(id);
                    }
                }
                _ => {}
            }
        }
        if !cores.is_empty() {
            self.state.physical = Some(cores.len());
        }
    }

    pub fn update(&mut self, now: Instant) {
        if util::read_into("/proc/stat", &mut self.buf).is_err() {
            self.state.usage =
                Reading::unavailable(Unit::Percent, Unavailable::Io("/proc/stat".into()));
            return;
        }
        // 借出緩衝區以避免同時可變/不可變借用。
        let text = std::mem::take(&mut self.buf);

        for line in text.lines() {
            if !line.starts_with("cpu") {
                break; // cpu* 一定在最前面，看到別的就可以停了
            }
            let Some((idx, t)) = parse_cpu_line(line) else {
                continue;
            };
            match idx {
                None => self.update_total(t),
                Some(i) => self.update_core(i, t),
            }
        }
        let counters = parse_stat_counters(&text);
        self.buf = text;

        self.apply_counters(counters, now);
        self.read_freqs();
        self.read_load();
        self.read_temps();
        self.state.uptime_secs = util::read_string("/proc/uptime")
            .ok()
            .and_then(|s| s.split_ascii_whitespace().next()?.parse().ok())
            .unwrap_or(0.0);
    }

    fn update_total(&mut self, t: CpuTimes) {
        if let Some(p) = self.total_delta.update(t.total(), t.idle_all()) {
            self.state.usage = Reading::exact(p, Unit::Percent).with_id("cpu.usage");
            self.state.history.push(p);
        }
        if self.primed {
            let dt = t.total().saturating_sub(self.prev_total.total());
            if dt > 0 {
                let iow =
                    t.iowait.saturating_sub(self.prev_total.iowait) as f64 / dt as f64 * 100.0;
                let stl = t.steal.saturating_sub(self.prev_total.steal) as f64 / dt as f64 * 100.0;
                self.state.iowait = Reading::exact(iow, Unit::Percent).with_id("cpu.iowait");
                self.state.steal = Reading::exact(stl, Unit::Percent).with_id("cpu.steal");
            }
        }
        self.prev_total = t;
        self.primed = true;
    }

    fn update_core(&mut self, i: usize, t: CpuTimes) {
        while self.state.cores.len() <= i {
            let index = self.state.cores.len();
            self.state.cores.push(Core {
                index,
                usage: Reading::unavailable(Unit::Percent, Unavailable::Pending),
                freq_mhz: Reading::unsupported(Unit::Megahertz),
                history: Series::new(self.history_len),
                delta: CpuDelta::default(),
            });
        }
        let core = &mut self.state.cores[i];
        if let Some(p) = core.delta.update(t.total(), t.idle_all()) {
            core.usage = Reading::exact(p, Unit::Percent).with_id("cpu.usage");
            core.history.push(p);
        }
        self.state.logical = self.state.cores.len();
    }

    fn apply_counters(&mut self, c: StatCounters, now: Instant) {
        let rate = |ctr: &mut Counter, v: Option<u64>| -> Reading {
            match v.and_then(|v| ctr.update(v, now)) {
                Some(r) => Reading::exact(r, Unit::CountPerSec),
                None => Reading::unavailable(Unit::CountPerSec, Unavailable::Pending),
            }
        };
        self.state.ctxt_rate = rate(&mut self.ctxt, c.ctxt).with_id("cpu.ctxt");
        self.state.intr_rate = rate(&mut self.intr, c.intr).with_id("cpu.intr");
        self.state.fork_rate = rate(&mut self.forks, c.forks).with_id("cpu.forks");
        self.state.running = Reading::from_opt(c.procs_running.map(|v| v as f64), Unit::Count)
            .with_id("cpu.running_blocked");
        self.state.blocked = Reading::from_opt(c.procs_blocked.map(|v| v as f64), Unit::Count)
            .with_id("cpu.running_blocked");
    }

    fn read_freqs(&mut self) {
        let mut any = false;
        for core in &mut self.state.cores {
            let p = format!(
                "/sys/devices/system/cpu/cpu{}/cpufreq/scaling_cur_freq",
                core.index
            );
            match util::read_i64(&p) {
                Ok(khz) => {
                    core.freq_mhz =
                        Reading::exact(khz as f64 / 1000.0, Unit::Megahertz).with_id("cpu.freq");
                    any = true;
                }
                Err(e) => core.freq_mhz = Reading::unavailable(Unit::Megahertz, e),
            }
        }
        // cpufreq 不存在時（虛擬機常見）退回 /proc/cpuinfo 的 cpu MHz。
        if !any {
            if let Ok(text) = util::read_string("/proc/cpuinfo") {
                let mut i = 0;
                for line in text.lines() {
                    let Some((k, v)) = line.split_once(':') else {
                        continue;
                    };
                    if k.trim() != "cpu MHz" {
                        continue;
                    }
                    if let (Some(core), Ok(mhz)) = (self.state.cores.get_mut(i), v.trim().parse()) {
                        core.freq_mhz = Reading::exact(mhz, Unit::Megahertz).with_id("cpu.freq");
                    }
                    i += 1;
                }
            }
        }
    }

    fn read_load(&mut self) {
        if let Some(l) = util::read_string("/proc/loadavg")
            .ok()
            .as_deref()
            .and_then(parse_loadavg)
        {
            self.state.load = l;
        }
    }

    fn read_temps(&mut self) {
        self.state.core_temps.clear();
        if self.state.topology.len() != self.state.logical {
            self.state.topology = read_topology(self.state.logical);
        }
        if self.hwmon.is_empty() {
            // 沒有 coretemp 就退回 thermal_zone0，但那通常是主機板溫度而非 CPU。
            self.state.temp = match util::read_i64("/sys/class/thermal/thermal_zone0/temp") {
                Ok(v) => {
                    Reading::estimated(v as f64 / 1000.0, Unit::Celsius, "ACPI thermal zone 0")
                        .with_id("cpu.temp")
                }
                Err(e) => Reading::unavailable(Unit::Celsius, e),
            };
            return;
        }
        let mut package = None;
        let mut max_core = f64::MIN;
        for dir in self.hwmon.clone() {
            // 一顆 CPU 封裝一個 coretemp 裝置。先掃一遍找出這個裝置屬於哪個
            // 封裝，`Core N` 才知道要掛在哪一顆封裝底下 —— 雙路機器上
            // 兩個封裝都會有 `Core 0`。
            let labels = read_hwmon_labels(&dir);
            let pkg_id = labels
                .iter()
                .find_map(|(l, _)| l.strip_prefix("Package id "))
                .and_then(|n| n.trim().parse::<u32>().ok())
                .unwrap_or(0);
            for (label, c) in labels {
                if label.contains("Package") || label.contains("Tdie") || label.contains("Tctl") {
                    // 多個封裝時取最熱的那顆當代表值
                    package = Some(package.map_or(c, |p: f64| p.max(c)));
                } else if let Some(n) = label
                    .strip_prefix("Core ")
                    .and_then(|n| n.trim().parse::<u32>().ok())
                {
                    self.state.core_temps.push((
                        CoreId {
                            package: pkg_id,
                            core: n,
                        },
                        c,
                    ));
                    max_core = max_core.max(c);
                }
            }
        }
        self.state
            .core_temps
            .sort_unstable_by_key(|(id, _)| (id.package, id.core));
        self.state.temp = match package.or(if max_core > f64::MIN {
            Some(max_core)
        } else {
            None
        }) {
            Some(c) => Reading::exact(c, Unit::Celsius).with_id("cpu.temp"),
            None => Reading::unavailable(Unit::Celsius, Unavailable::Unsupported),
        };
    }
}

/// 讀出一個 hwmon 裝置裡所有帶 label 的溫度感測器。
fn read_hwmon_labels(dir: &std::path::Path) -> Vec<(String, f64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut v = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(idx) = name
            .strip_suffix("_label")
            .filter(|s| s.starts_with("temp"))
        else {
            continue;
        };
        let (Ok(label), Ok(milli)) = (
            util::read_trimmed(dir.join(name)),
            util::read_i64(dir.join(format!("{idx}_input"))),
        ) else {
            continue;
        };
        v.push((label, milli as f64 / 1000.0));
    }
    v
}

/// 找出所有 CPU 的 hwmon 目錄。hwmon 編號開機後會變，所以每次都要靠 name 比對。
///
/// 回傳全部而不是第一個：雙路機器上 coretemp 一個封裝註冊一個裝置，
/// 只取第一個會漏掉另一顆 CPU 的全部核心溫度。
fn find_cpu_hwmons() -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir("/sys/class/hwmon") else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = rd.flatten().collect();
    dirs.sort_by_key(|e| e.file_name());
    dirs.into_iter()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                util::read_trimmed(p.join("name")).as_deref(),
                Ok("coretemp") | Ok("k10temp") | Ok("zenpower")
            )
        })
        .collect()
}

impl CpuState {
    /// 這顆邏輯處理器所在實體核心的溫度。
    ///
    /// 感測器是掛在實體核心上的，所以超執行緒的兩顆邏輯處理器會拿到同一個值 ——
    /// 那是實話：它們本來就共用同一塊矽。拓樸讀不到時回 `None`，
    /// 不猜、也不拿別顆核心的溫度充數。
    pub fn core_temp(&self, logical: usize) -> Option<f64> {
        let id = self.topology.get(logical)?;
        self.core_temps
            .iter()
            .find(|(c, _)| c == id)
            .map(|(_, t)| *t)
    }

    /// 有幾顆邏輯處理器對得到溫度感測器。
    pub fn cores_with_temp(&self) -> usize {
        (0..self.logical)
            .filter(|&i| self.core_temp(i).is_some())
            .count()
    }

    pub fn freq_avg(&self) -> Reading {
        let vals: Vec<f64> = self.cores.iter().filter_map(|c| c.freq_mhz.get()).collect();
        if vals.is_empty() {
            return Reading::unsupported(Unit::Megahertz).with_id("cpu.freq");
        }
        Reading::exact(
            vals.iter().sum::<f64>() / vals.len() as f64,
            Unit::Megahertz,
        )
        .with_id("cpu.freq")
    }
    /// load 除以核心數 —— 這才是真正的滿載程度。
    pub fn load_per_core(&self) -> f64 {
        self.load[0] / self.logical.max(1) as f64
    }
    pub fn temp_quality(&self) -> &Quality {
        &self.temp.quality
    }
}

#[cfg(test)]
mod tests {
    // 溫度感測器掛在實體核心上，label 寫的是 core_id。
    // i7-13700 的 P-core core_id 是 0/4/8/…，直接當成 cpuN 只會對上四分之一。
    #[test]
    fn core_temperature_maps_through_topology_not_cpu_number() {
        use super::{CoreId, CpuState};
        let mut st = CpuState {
            logical: 6,
            ..Default::default()
        };
        // 3 顆實體核心（core_id 0 / 4 / 8），每顆兩條執行緒
        st.topology = vec![0u32, 0, 4, 4, 8, 8]
            .into_iter()
            .map(|core| CoreId { package: 0, core })
            .collect();
        st.core_temps = vec![
            (
                CoreId {
                    package: 0,
                    core: 0,
                },
                40.0,
            ),
            (
                CoreId {
                    package: 0,
                    core: 4,
                },
                50.0,
            ),
            (
                CoreId {
                    package: 0,
                    core: 8,
                },
                60.0,
            ),
        ];
        assert_eq!(st.core_temp(0), Some(40.0));
        assert_eq!(st.core_temp(1), Some(40.0), "同一顆核心的兩條執行緒同溫");
        assert_eq!(st.core_temp(2), Some(50.0));
        assert_eq!(st.core_temp(3), Some(50.0));
        assert_eq!(st.core_temp(4), Some(60.0));
        assert_eq!(st.core_temp(5), Some(60.0));
        assert_eq!(
            st.cores_with_temp(),
            6,
            "每顆邏輯處理器都該對得到溫度，不是只有 core_id 撞上編號的那幾顆"
        );
        assert_eq!(st.core_temp(6), None, "不存在的核心不該回傳溫度");
    }

    #[test]
    fn core_temperature_is_none_without_topology() {
        use super::{CoreId, CpuState};
        let st = CpuState {
            logical: 4,
            core_temps: vec![(
                CoreId {
                    package: 0,
                    core: 0,
                },
                40.0,
            )],
            ..Default::default()
        };
        // 拓樸讀不到時寧可不顯示，也不要把某顆核心的溫度標到別顆頭上
        assert_eq!(st.core_temp(0), None);
        assert_eq!(st.cores_with_temp(), 0);
    }

    #[test]
    fn two_sockets_do_not_share_core_ids() {
        use super::{CoreId, CpuState};
        let mut st = CpuState {
            logical: 4,
            ..Default::default()
        };
        st.topology = vec![
            CoreId {
                package: 0,
                core: 0,
            },
            CoreId {
                package: 0,
                core: 1,
            },
            CoreId {
                package: 1,
                core: 0,
            },
            CoreId {
                package: 1,
                core: 1,
            },
        ];
        st.core_temps = vec![
            (
                CoreId {
                    package: 0,
                    core: 0,
                },
                40.0,
            ),
            (
                CoreId {
                    package: 0,
                    core: 1,
                },
                41.0,
            ),
            (
                CoreId {
                    package: 1,
                    core: 0,
                },
                70.0,
            ),
            (
                CoreId {
                    package: 1,
                    core: 1,
                },
                71.0,
            ),
        ];
        // 兩個封裝都有 core 0，不能混在一起
        assert_eq!(st.core_temp(0), Some(40.0));
        assert_eq!(st.core_temp(2), Some(70.0));
    }

    use super::*;

    #[test]
    fn parses_aggregate_cpu_line() {
        let (idx, t) = parse_cpu_line("cpu  100 20 30 4000 50 6 7 8 9 10").unwrap();
        assert_eq!(idx, None);
        assert_eq!(t.user, 100);
        assert_eq!(t.idle, 4000);
        assert_eq!(t.iowait, 50);
        assert_eq!(t.steal, 8);
        // guest/guest_nice（9、10）不可重複計入
        assert_eq!(t.total(), 100 + 20 + 30 + 4000 + 50 + 6 + 7 + 8);
        assert_eq!(t.idle_all(), 4050);
    }

    #[test]
    fn parses_per_core_line() {
        let (idx, t) = parse_cpu_line("cpu11 1 2 3 4 5 6 7 8").unwrap();
        assert_eq!(idx, Some(11));
        assert_eq!(t.user, 1);
    }

    #[test]
    fn tolerates_old_kernel_without_steal() {
        let (_, t) = parse_cpu_line("cpu 1 2 3 4").unwrap();
        assert_eq!(t.steal, 0);
        assert_eq!(t.iowait, 0);
        assert_eq!(t.total(), 10);
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(parse_cpu_line("").is_none());
        assert!(parse_cpu_line("intr 1 2 3").is_none());
        assert!(
            parse_cpu_line("cpu 1 2").is_none(),
            "欄位不足 4 個應視為不合法"
        );
        assert!(parse_cpu_line("cpuX 1 2 3 4").is_none());
    }

    #[test]
    fn ignores_non_numeric_fields() {
        let (_, t) = parse_cpu_line("cpu 1 x 3 4 5").unwrap();
        // 非數字欄位被略過，後面的欄位會往前遞補；重點是不 panic
        assert_eq!(t.user, 1);
    }

    #[test]
    fn parses_stat_counters() {
        let c = parse_stat_counters(
            "cpu  1 2 3 4\nintr 12345 0 0\nctxt 99999\nbtime 1700000000\n\
             processes 4242\nprocs_running 3\nprocs_blocked 1\n",
        );
        assert_eq!(c.ctxt, Some(99999));
        assert_eq!(c.intr, Some(12345));
        assert_eq!(c.forks, Some(4242));
        assert_eq!(c.procs_running, Some(3));
        assert_eq!(c.procs_blocked, Some(1));
    }

    #[test]
    fn missing_counters_are_none_not_zero() {
        let c = parse_stat_counters("cpu 1 2 3 4\n");
        assert_eq!(c.ctxt, None, "缺欄位要是 None，不能當成 0");
    }

    #[test]
    fn parses_loadavg() {
        assert_eq!(
            parse_loadavg("1.05 0.98 1.10 2/1234 5678"),
            Some([1.05, 0.98, 1.10])
        );
        assert_eq!(parse_loadavg(""), None);
        assert_eq!(parse_loadavg("1.0 2.0"), None);
    }

    #[test]
    fn cpu_delta_first_sample_has_no_value() {
        let mut d = CpuDelta::default();
        assert_eq!(
            d.update(1000, 900),
            None,
            "第一次取樣沒有基準點，不能回傳數字"
        );
        assert_eq!(d.update(2000, 1400), Some(50.0));
    }

    #[test]
    fn cpu_delta_handles_counter_reset() {
        let mut d = CpuDelta::default();
        d.update(1000, 900);
        assert_eq!(d.update(500, 400), None, "計數器倒退時不可回傳荒謬數值");
    }

    #[test]
    fn cpu_delta_clamps_to_valid_range() {
        let mut d = CpuDelta::default();
        d.update(1000, 0);
        // idle 增加得比 total 還多（不該發生，但要能撐住）
        let v = d.update(1100, 5000).unwrap();
        assert!(
            (0.0..=100.0).contains(&v),
            "使用率必須在 0–100 之間，得到 {v}"
        );
    }
}
