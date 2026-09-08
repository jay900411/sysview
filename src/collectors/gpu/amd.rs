//! AMD backend（amdgpu），走 DRM sysfs。
//!
//! amdgpu 比 i915 大方：它直接提供 `gpu_busy_percent`，所以這裡的使用率是
//! **精確值**而不是估計值。VRAM 也有 `mem_info_vram_*` 可讀。
//!
//! ⚠ 這個 backend 是照 amdgpu 的 sysfs ABI 實作的，但**沒有實機驗證過**
//! （開發機上只有 NVIDIA + Intel）。README 會據實標示為 implemented but unverified。

use std::path::PathBuf;

use crate::collectors::gpu::{none, GpuDevice, Vendor};
use crate::collectors::util;
use crate::metrics::model::{Reading, Unit};

struct Card {
    id: String,
    path: PathBuf,
    name: String,
    hwmon: Option<PathBuf>,
}

pub struct AmdBackend {
    cards: Vec<Card>,
}

impl AmdBackend {
    pub fn new() -> Self {
        Self { cards: discover() }
    }
    pub fn poll(&mut self) -> Vec<GpuDevice> {
        self.cards.iter().map(read_card).collect()
    }
}

impl Default for AmdBackend {
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
            if Vendor::from_pci(&vendor) != Some(Vendor::Amd) {
                return None;
            }
            let devid = util::read_trimmed(path.join("device/device")).unwrap_or_default();
            let hwmon = find_hwmon(&path);
            Some(Card {
                name: format!("AMD GPU {devid}"),
                id,
                path,
                hwmon,
            })
        })
        .collect()
}

/// amdgpu 的溫度/功耗掛在 `device/hwmon/hwmonN/` 底下。
fn find_hwmon(card: &std::path::Path) -> Option<PathBuf> {
    let dir = std::fs::read_dir(card.join("device/hwmon")).ok()?;
    dir.flatten().map(|e| e.path()).next()
}

fn read_card(c: &Card) -> GpuDevice {
    let dev = c.path.join("device");
    // amdgpu 直接提供忙碌率 —— 這是精確值，不是估計
    let busy = util::read_i64_opt(dev.join("gpu_busy_percent"));
    let vram_total = util::read_i64_opt(dev.join("mem_info_vram_total"));
    let vram_used = util::read_i64_opt(dev.join("mem_info_vram_used"));

    let (temp, power, fan) = match &c.hwmon {
        Some(h) => (
            util::read_i64_opt(h.join("temp1_input")).map(|v| v as f64 / 1000.0),
            // average_power 單位是微瓦
            util::read_i64_opt(h.join("power1_average")).map(|v| v as f64 / 1e6),
            util::read_i64_opt(h.join("fan1_input")).map(|v| v as f64),
        ),
        None => (None, None, None),
    };

    GpuDevice {
        id: c.id.clone(),
        vendor: Vendor::Amd,
        name: c.name.clone(),
        backend: "amdgpu DRM sysfs",
        utilization: match busy {
            Some(b) => Reading::exact(b as f64, Unit::Percent).with_id("gpu.util"),
            None => none(Unit::Percent).with_id("gpu.util"),
        },
        memory_used: opt(vram_used.map(|v| v as f64), Unit::Bytes, "gpu.vram"),
        memory_total: opt(vram_total.map(|v| v as f64), Unit::Bytes, "gpu.vram"),
        memory_utilization: none(Unit::Percent),
        temperature: opt(temp, Unit::Celsius, "gpu.temp"),
        power: opt(power, Unit::Watts, "gpu.power"),
        power_limit: opt(
            c.hwmon
                .as_ref()
                .and_then(|h| util::read_i64_opt(h.join("power1_cap")))
                .map(|v| v as f64 / 1e6),
            Unit::Watts,
            "gpu.power",
        ),
        sm_clock: opt(
            util::read_i64_opt(c.path.join("device/pp_dpm_sclk")).map(|v| v as f64),
            Unit::Megahertz,
            "gpu.clock",
        ),
        mem_clock: none(Unit::Megahertz),
        fan: opt(fan, Unit::Count, "gpu.fan"),
        pstate: util::read_trimmed(dev.join("power_dpm_force_performance_level")).ok(),
        clock_range: None,
        driver_version: None,
        pcie_gen: util::read_i64_opt(dev.join("current_link_speed")).map(|v| v as u32),
        pcie_width: util::read_i64_opt(dev.join("current_link_width")).map(|v| v as u32),
        processes: Vec::new(),
        process_list_complete: false,
    }
}

fn opt(v: Option<f64>, unit: Unit, id: &'static str) -> Reading {
    match v {
        Some(v) => Reading::exact(v, unit).with_id(id),
        None => none(unit).with_id(id),
    }
}

/// 這個 backend 沒有實機驗證過，UI 與 README 會據實標示。
pub const VERIFIED_ON_HARDWARE: bool = false;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_exact_utilization_not_estimated() {
        let t = tempfile::tempdir().unwrap();
        let dev = t.path().join("device");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("gpu_busy_percent"), "42\n").unwrap();
        std::fs::write(dev.join("mem_info_vram_total"), "17179869184\n").unwrap();
        std::fs::write(dev.join("mem_info_vram_used"), "4294967296\n").unwrap();
        let c = Card {
            id: "card0".into(),
            path: t.path().to_path_buf(),
            name: "AMD test".into(),
            hwmon: None,
        };
        let d = read_card(&c);
        assert_eq!(d.utilization.get(), Some(42.0));
        assert!(
            !d.utilization.quality.is_estimated(),
            "amdgpu 有 gpu_busy_percent，是精確值不是估計值"
        );
        assert_eq!(d.memory_percent(), Some(25.0));
    }

    #[test]
    fn missing_sysfs_nodes_report_unavailable_not_zero() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("device")).unwrap();
        let c = Card {
            id: "card0".into(),
            path: t.path().to_path_buf(),
            name: "x".into(),
            hwmon: None,
        };
        let d = read_card(&c);
        assert!(!d.utilization.quality.is_available(), "讀不到不可以當成 0%");
        assert!(!d.temperature.quality.is_available());
        assert_eq!(d.memory_percent(), None);
    }

    #[test]
    fn amd_backend_is_marked_unverified() {
        // 這是刻意的常數斷言：提醒任何想改成 true 的人先真的拿到 AMD 卡驗證
        #[allow(clippy::assertions_on_constants)]
        {
            assert!(!VERIFIED_ON_HARDWARE, "沒有實機驗證過就不能宣稱已驗證");
        }
    }
}
