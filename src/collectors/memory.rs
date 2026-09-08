//! Memory collector：`/proc/meminfo`。

use std::collections::HashMap;

use crate::collectors::util;
use crate::error::Unavailable;
use crate::metrics::model::{Reading, Series, Unit};

/// 解析 `/proc/meminfo`。所有值一律換算成**位元組**。
///
/// 格式是 `Key:   12345 kB`，少數欄位（HugePages_*）沒有單位後綴，
/// 那些是「個數」不是 kB，所以不能無條件乘 1024。
pub fn parse_meminfo(text: &str) -> HashMap<String, u64> {
    let mut out = HashMap::with_capacity(64);
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let mut it = rest.split_ascii_whitespace();
        let Some(num) = it.next().and_then(|v| v.parse::<u64>().ok()) else {
            continue;
        };
        let scaled = match it.next() {
            Some("kB") | Some("KB") => num.saturating_mul(1024),
            // 沒有單位 = 個數（HugePages_Total 之類），保持原值
            _ => num,
        };
        out.insert(key.to_owned(), scaled);
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct MemState {
    pub info: HashMap<String, u64>,
    pub history: Series,
    pub swap_history: Series,
}

impl MemState {
    pub fn get(&self, k: &str) -> Option<u64> {
        self.info.get(k).copied()
    }
    fn g0(&self, k: &str) -> u64 {
        self.get(k).unwrap_or(0)
    }

    pub fn total(&self) -> u64 {
        self.g0("MemTotal")
    }
    /// MemAvailable 是核心估的「不觸發 swap 還能給新程式用多少」，
    /// 這才是該看的數字。舊核心沒有這欄時退回 MemFree（會偏保守）。
    pub fn available(&self) -> u64 {
        self.get("MemAvailable")
            .unwrap_or_else(|| self.g0("MemFree"))
    }
    pub fn used(&self) -> u64 {
        self.total().saturating_sub(self.available())
    }
    pub fn used_percent(&self) -> Reading {
        let t = self.total();
        if t == 0 {
            return Reading::unavailable(
                Unit::Percent,
                Unavailable::Malformed("MemTotal=0".into()),
            );
        }
        Reading::exact(self.used() as f64 / t as f64 * 100.0, Unit::Percent).with_id("mem.used")
    }
    /// 可回收的 page cache。Shmem/tmpfs 算在 Cached 裡但**不能**回收，要扣掉。
    pub fn reclaimable_cache(&self) -> u64 {
        self.g0("Cached")
            .saturating_add(self.g0("SReclaimable"))
            .saturating_sub(self.g0("Shmem"))
    }
    pub fn buffers(&self) -> u64 {
        self.g0("Buffers")
    }
    pub fn free(&self) -> u64 {
        self.g0("MemFree")
    }
    pub fn swap_total(&self) -> u64 {
        self.g0("SwapTotal")
    }
    pub fn swap_used(&self) -> u64 {
        self.swap_total().saturating_sub(self.g0("SwapFree"))
    }
    pub fn swap_percent(&self) -> Reading {
        let t = self.swap_total();
        if t == 0 {
            return Reading::unavailable(Unit::Percent, Unavailable::Unsupported)
                .with_id("mem.swap");
        }
        Reading::exact(self.swap_used() as f64 / t as f64 * 100.0, Unit::Percent)
            .with_id("mem.swap")
    }
}

pub struct MemCollector {
    state: MemState,
    buf: String,
}

impl MemCollector {
    pub fn new(history_len: usize) -> Self {
        Self {
            state: MemState {
                info: HashMap::new(),
                history: Series::new(history_len),
                swap_history: Series::new(history_len),
            },
            buf: String::with_capacity(4096),
        }
    }
    pub fn state(&self) -> &MemState {
        &self.state
    }
    pub fn update(&mut self) {
        if util::read_into("/proc/meminfo", &mut self.buf).is_err() {
            return;
        }
        self.state.info = parse_meminfo(&self.buf);
        if let Some(p) = self.state.used_percent().get() {
            self.state.history.push(p);
        }
        self.state
            .swap_history
            .push(self.state.swap_percent().get().unwrap_or(0.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
MemTotal:       65536000 kB
MemFree:         1024000 kB
MemAvailable:   58000000 kB
Buffers:         4096000 kB
Cached:         13000000 kB
SwapCached:            0 kB
SwapTotal:      35000000 kB
SwapFree:       35000000 kB
Shmem:            350000 kB
SReclaimable:    2000000 kB
HugePages_Total:       0
HugePages_Free:        0
";

    #[test]
    fn converts_kb_to_bytes() {
        let m = parse_meminfo(SAMPLE);
        assert_eq!(m["MemTotal"], 65_536_000 * 1024);
        assert_eq!(m["MemAvailable"], 58_000_000 * 1024);
    }

    #[test]
    fn unitless_fields_are_not_scaled() {
        let m = parse_meminfo("HugePages_Total:   7\n");
        assert_eq!(m["HugePages_Total"], 7, "個數欄位不可乘 1024");
    }

    #[test]
    fn computes_used_from_available_not_free() {
        let s = MemState {
            info: parse_meminfo(SAMPLE),
            ..Default::default()
        };
        // used = total - available，跟 free(1) 的 used 欄定義一致
        assert_eq!(s.used(), (65_536_000 - 58_000_000) * 1024);
        assert!(s.used() < s.total() - s.free(), "不可用 MemFree 算 used");
    }

    #[test]
    fn cache_excludes_shmem_because_it_is_not_reclaimable() {
        let s = MemState {
            info: parse_meminfo(SAMPLE),
            ..Default::default()
        };
        assert_eq!(
            s.reclaimable_cache(),
            (13_000_000 + 2_000_000 - 350_000) * 1024
        );
    }

    #[test]
    fn falls_back_to_memfree_on_old_kernels() {
        let s = MemState {
            info: parse_meminfo("MemTotal: 1000 kB\nMemFree: 400 kB\n"),
            ..Default::default()
        };
        assert_eq!(
            s.available(),
            400 * 1024,
            "沒有 MemAvailable 時退回 MemFree"
        );
    }

    #[test]
    fn swap_percent_is_unsupported_when_no_swap() {
        let s = MemState {
            info: parse_meminfo("MemTotal: 1000 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n"),
            ..Default::default()
        };
        assert!(
            !s.swap_percent().quality.is_available(),
            "沒有 swap 應標示為不支援而非 0%"
        );
    }

    #[test]
    fn empty_input_does_not_panic() {
        let m = parse_meminfo("");
        assert!(m.is_empty());
        let s = MemState::default();
        assert_eq!(s.total(), 0);
        assert!(
            !s.used_percent().quality.is_available(),
            "MemTotal=0 應回報不可用而不是除以零"
        );
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let m = parse_meminfo("garbage\nMemTotal: notanumber kB\nMemFree: 100 kB\n");
        assert!(!m.contains_key("MemTotal"));
        assert_eq!(m["MemFree"], 100 * 1024);
    }

    #[test]
    fn saturating_arithmetic_survives_inconsistent_snapshot() {
        // meminfo 不是原子快照，欄位之間可能互相矛盾
        let s = MemState {
            info: parse_meminfo("MemTotal: 100 kB\nMemAvailable: 999999 kB\n"),
            ..Default::default()
        };
        assert_eq!(s.used(), 0, "available > total 時不可下溢");
    }
}
