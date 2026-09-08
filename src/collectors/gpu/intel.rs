//! Intel 內顯 backend（i915 / xe），走 DRM sysfs。
//!
//! **重點：i915 沒有 `gpu_busy_percent` 這種介面。**
//! 唯一免 root 的近似方法是讀 `power/rc6_residency_ms` —— GPU 進入 RC6
//! 省電休眠的累計毫秒數 —— 再用「沒在睡的比例」當忙碌比例。
//!
//! 這是**估計值**，UI 會標成 `estimated`，Explain 頁會說明算法與限制。
//! 絕不假裝它是精確值。要精確數字得用 `intel_gpu_top`，但那需要 root
//! 或 `CAP_PERFMON`，不適合放進一個所有人都能跑的工具裡。

use std::path::PathBuf;
use std::time::Instant;

use crate::collectors::gpu::{none, GpuDevice, Vendor};
use crate::collectors::util;
use crate::metrics::model::{Reading, Unit};

/// RC6 反推法的名稱，會顯示在 UI 的 estimated 標記與 Explain 裡。
const RC6_METHOD: &str = "RC6 residency (idle-time inversion)";

struct Card {
    id: String,
    path: PathBuf,
    name: String,
    /// 上次的 RC6 累計值與取樣時刻，用來算差分。
    prev_rc6: Option<(i64, Instant)>,
    last_util: Option<f64>,
}

pub struct IntelBackend {
    cards: Vec<Card>,
}

impl IntelBackend {
    pub fn new() -> Self {
        Self { cards: discover() }
    }

    pub fn poll(&mut self, now: Instant) -> Vec<GpuDevice> {
        self.cards.iter_mut().map(|c| read_card(c, now)).collect()
    }
}

impl Default for IntelBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn discover() -> Vec<Card> {
    let Ok(dir) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut names: Vec<String> = dir
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            // 只要 cardN，不要 card1-HDMI-A-1 這種 connector
            let rest = n.strip_prefix("card")?;
            (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())).then_some(n)
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|id| {
            let path = PathBuf::from("/sys/class/drm").join(&id);
            let vendor = util::read_trimmed(path.join("device/vendor")).ok()?;
            if Vendor::from_pci(&vendor) != Some(Vendor::Intel) {
                return None;
            }
            let devid = util::read_trimmed(path.join("device/device")).unwrap_or_default();
            Some(Card {
                name: model_name(&devid),
                id,
                path,
                prev_rc6: None,
                last_util: None,
            })
        })
        .collect()
}

fn read_card(c: &mut Card, now: Instant) -> GpuDevice {
    // gt_act_freq_mhz 是實際頻率；GPU 睡著時會是 0（這是真的，不是讀取失敗）。
    let freq = util::read_i64_opt(c.path.join("gt_act_freq_mhz"))
        .or_else(|| util::read_i64_opt(c.path.join("gt_cur_freq_mhz")));
    let fmax = util::read_i64_opt(c.path.join("gt_max_freq_mhz"))
        .or_else(|| util::read_i64_opt(c.path.join("gt_RP0_freq_mhz")));
    let fmin = util::read_i64_opt(c.path.join("gt_min_freq_mhz"))
        .or_else(|| util::read_i64_opt(c.path.join("gt_RPn_freq_mhz")));

    let utilization = match compute_rc6_utilization(c, now) {
        Some(u) => Reading::estimated(u, Unit::Percent, RC6_METHOD).with_id("gpu.intel_util"),
        // 拿不到就是拿不到 —— 不用頻率比例之類的東西瞎編一個數字
        None => none(Unit::Percent).with_id("gpu.intel_util"),
    };

    GpuDevice {
        id: c.id.clone(),
        vendor: Vendor::Intel,
        name: c.name.clone(),
        backend: "i915 DRM sysfs (RC6 estimate)",
        utilization,
        memory_used: none(Unit::Bytes),
        memory_total: none(Unit::Bytes),
        memory_utilization: none(Unit::Percent),
        temperature: none(Unit::Celsius),
        power: none(Unit::Watts),
        power_limit: none(Unit::Watts),
        sm_clock: match freq {
            Some(f) => Reading::exact(f as f64, Unit::Megahertz).with_id("gpu.clock"),
            None => none(Unit::Megahertz).with_id("gpu.clock"),
        },
        mem_clock: none(Unit::Megahertz),
        fan: none(Unit::Percent),
        pstate: None,
        clock_range: match (fmin, fmax) {
            (Some(a), Some(b)) => Some(format!("{a} – {b} MHz")),
            _ => None,
        },
        driver_version: None,
        pcie_gen: None,
        pcie_width: None,
        processes: Vec::new(),
        process_list_complete: false,
    }
}

/// 由 RC6 殘留時間反推忙碌比例。
///
/// `rc6_residency_ms` 是「GPU 處於 RC6 省電狀態的累計毫秒數」。
/// 兩次取樣之間，若經過 1000 ms 而 RC6 只增加 200 ms，
/// 代表有 800 ms 醒著 → 忙碌率約 80%。
///
/// 第一次呼叫只建立基準點，回 `None`（沒有基準就沒有差分可算）。
fn compute_rc6_utilization(c: &mut Card, now: Instant) -> Option<f64> {
    let rc6 = util::read_i64_opt(c.path.join("power/rc6_residency_ms"))?;
    let Some((prev_rc6, prev_t)) = c.prev_rc6 else {
        c.prev_rc6 = Some((rc6, now));
        return None;
    };
    let elapsed_ms = now.duration_since(prev_t).as_secs_f64() * 1000.0;
    // 間隔太短的話量測誤差會蓋過訊號，沿用上一次的值。
    if elapsed_ms < 50.0 {
        return c.last_util;
    }
    // 計數器不應倒退；若倒退（模組重載）就重建基準點。
    if rc6 < prev_rc6 {
        c.prev_rc6 = Some((rc6, now));
        return None;
    }
    let idle_ms = (rc6 - prev_rc6) as f64;
    let util = (100.0 - idle_ms / elapsed_ms * 100.0).clamp(0.0, 100.0);
    c.prev_rc6 = Some((rc6, now));
    c.last_util = Some(util);
    Some(util)
}

/// 已知的 Intel 顯示晶片。查不到就顯示 PCI device id，不亂猜型號。
fn model_name(devid: &str) -> String {
    const KNOWN: &[(&str, &str)] = &[
        ("0xa780", "UHD Graphics 770 (Raptor Lake-S)"),
        ("0xa788", "UHD Graphics 730 (Raptor Lake-S)"),
        ("0x4680", "UHD Graphics 770 (Alder Lake-S)"),
        ("0x4692", "UHD Graphics 730 (Alder Lake-S)"),
        ("0x9a49", "Iris Xe Graphics (Tiger Lake)"),
        ("0x46a6", "Iris Xe Graphics (Alder Lake-P)"),
        ("0x3e92", "UHD Graphics 630 (Coffee Lake)"),
        ("0x5917", "UHD Graphics 620 (Kaby Lake R)"),
        ("0x7d55", "Arc Graphics (Meteor Lake)"),
    ];
    KNOWN
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(devid))
        .map(|(_, n)| (*n).to_owned())
        .unwrap_or_else(|| format!("Intel Graphics {devid}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn card(dir: &std::path::Path) -> Card {
        Card {
            id: "card0".into(),
            path: dir.to_path_buf(),
            name: "test".into(),
            prev_rc6: None,
            last_util: None,
        }
    }

    fn write_rc6(dir: &std::path::Path, v: i64) {
        let p = dir.join("power");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("rc6_residency_ms"), format!("{v}\n")).unwrap();
    }

    #[test]
    fn first_sample_establishes_baseline_and_returns_none() {
        let t = tempfile::tempdir().unwrap();
        write_rc6(t.path(), 1000);
        let mut c = card(t.path());
        let now = Instant::now();
        assert_eq!(
            compute_rc6_utilization(&mut c, now),
            None,
            "沒有基準點就不能報數字"
        );
        assert!(c.prev_rc6.is_some(), "第一次呼叫必須種下基準點");
    }

    #[test]
    fn fully_idle_gpu_reports_zero() {
        let t = tempfile::tempdir().unwrap();
        write_rc6(t.path(), 1000);
        let mut c = card(t.path());
        let t0 = Instant::now();
        compute_rc6_utilization(&mut c, t0);
        // 經過 1000ms，RC6 也增加 1000ms → 一直在睡 → 0% 忙碌
        write_rc6(t.path(), 2000);
        let u = compute_rc6_utilization(&mut c, t0 + Duration::from_millis(1000)).unwrap();
        assert!(u.abs() < 1.0, "完全閒置應接近 0%，得到 {u}");
    }

    #[test]
    fn fully_busy_gpu_reports_hundred() {
        let t = tempfile::tempdir().unwrap();
        write_rc6(t.path(), 5000);
        let mut c = card(t.path());
        let t0 = Instant::now();
        compute_rc6_utilization(&mut c, t0);
        write_rc6(t.path(), 5000); // 完全沒進入 RC6
        let u = compute_rc6_utilization(&mut c, t0 + Duration::from_millis(1000)).unwrap();
        assert!(u > 99.0, "完全沒睡應接近 100%，得到 {u}");
    }

    #[test]
    fn partial_load_is_proportional() {
        let t = tempfile::tempdir().unwrap();
        write_rc6(t.path(), 0);
        let mut c = card(t.path());
        let t0 = Instant::now();
        compute_rc6_utilization(&mut c, t0);
        write_rc6(t.path(), 250); // 1000ms 中睡了 250ms
        let u = compute_rc6_utilization(&mut c, t0 + Duration::from_millis(1000)).unwrap();
        assert!((u - 75.0).abs() < 1.0, "應約為 75%，得到 {u}");
    }

    #[test]
    fn too_short_interval_reuses_last_value() {
        let t = tempfile::tempdir().unwrap();
        write_rc6(t.path(), 0);
        let mut c = card(t.path());
        let t0 = Instant::now();
        compute_rc6_utilization(&mut c, t0);
        write_rc6(t.path(), 500);
        let u1 = compute_rc6_utilization(&mut c, t0 + Duration::from_millis(1000)).unwrap();
        // 立刻再取樣一次：間隔 <50ms，應沿用而不是算出 100%
        let u2 = compute_rc6_utilization(&mut c, t0 + Duration::from_millis(1010)).unwrap();
        assert_eq!(u1, u2, "間隔過短時應沿用上次的值，避免噪音放大成 100%");
    }

    #[test]
    fn counter_going_backwards_rebuilds_baseline() {
        let t = tempfile::tempdir().unwrap();
        write_rc6(t.path(), 9000);
        let mut c = card(t.path());
        let t0 = Instant::now();
        compute_rc6_utilization(&mut c, t0);
        write_rc6(t.path(), 10); // 模組重載，計數器歸零
        assert_eq!(
            compute_rc6_utilization(&mut c, t0 + Duration::from_millis(1000)),
            None,
            "計數器倒退時應重建基準而不是回傳荒謬值"
        );
    }

    #[test]
    fn missing_rc6_file_yields_none_not_panic() {
        let t = tempfile::tempdir().unwrap();
        let mut c = card(t.path());
        assert_eq!(compute_rc6_utilization(&mut c, Instant::now()), None);
    }

    #[test]
    fn utilization_is_always_marked_estimated() {
        let t = tempfile::tempdir().unwrap();
        write_rc6(t.path(), 0);
        let mut c = card(t.path());
        let t0 = Instant::now();
        read_card(&mut c, t0);
        write_rc6(t.path(), 500);
        let d = read_card(&mut c, t0 + Duration::from_millis(1000));
        assert!(
            d.utilization.quality.is_estimated(),
            "Intel iGPU 使用率是推算出來的，必須標成 estimated"
        );
    }

    #[test]
    fn unknown_device_id_is_not_guessed() {
        let n = model_name("0xffff");
        assert!(
            n.contains("0xffff"),
            "查不到型號時應顯示 device id 而不是亂猜"
        );
        assert_eq!(model_name("0xa780"), "UHD Graphics 770 (Raptor Lake-S)");
    }
}
