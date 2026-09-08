//! hwmon 感測器的共用探索與讀取。
//!
//! `/sys/class/hwmon/hwmonN` 的編號**開機後可能改變**，所以任何時候都不能寫死
//! `hwmon0`，必須靠 `name` 檔案比對晶片。這個模組把探索做一次、快取路徑，
//! 之後每輪只重讀數值檔。

use std::path::{Path, PathBuf};

use crate::collectors::util;

/// 一顆 hwmon 晶片下的一個感測器。
#[derive(Debug, Clone)]
pub struct Sensor {
    /// 例如 "Package id 0"、"Composite"、"fan1"
    pub label: String,
    /// 讀值用的路徑，例如 `.../temp1_input`
    pub input: PathBuf,
    pub kind: SensorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorKind {
    /// 毫攝氏度
    Temp,
    /// RPM
    Fan,
    /// 微瓦
    Power,
}

impl SensorKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Temp => "temp",
            Self::Fan => "fan",
            Self::Power => "power",
        }
    }
    /// 把 sysfs 的原始整數換算成人看的單位。
    pub fn scale(self, raw: i64) -> f64 {
        match self {
            Self::Temp => raw as f64 / 1000.0, // 毫度 → 度
            Self::Fan => raw as f64,           // 已經是 RPM
            Self::Power => raw as f64 / 1e6,   // 微瓦 → 瓦
        }
    }
}

/// 一顆 hwmon 晶片。
#[derive(Debug, Clone)]
pub struct Chip {
    pub name: String,
    pub path: PathBuf,
    pub sensors: Vec<Sensor>,
}

impl Chip {
    /// 讀出某個感測器目前的值。裝置可能隨時消失，所以回 `Option`。
    pub fn read(&self, s: &Sensor) -> Option<f64> {
        util::read_i64(&s.input).ok().map(|v| s.kind.scale(v))
    }
    /// 找出第一個 label 符合條件的感測器並讀值。
    pub fn read_labeled(&self, pred: impl Fn(&str) -> bool) -> Option<f64> {
        self.sensors
            .iter()
            .find(|s| pred(&s.label))
            .and_then(|s| self.read(s))
    }
}

/// 掃描所有 hwmon 晶片。建議只在啟動時與偶爾（裝置熱插拔）呼叫。
pub fn scan() -> Vec<Chip> {
    let Ok(dir) = std::fs::read_dir("/sys/class/hwmon") else {
        return Vec::new();
    };
    let mut entries: Vec<_> = dir.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    entries
        .into_iter()
        .filter_map(|e| {
            let path = e.path();
            let name = util::read_trimmed(path.join("name")).ok()?;
            let sensors = scan_sensors(&path);
            Some(Chip {
                name,
                path,
                sensors,
            })
        })
        .collect()
}

fn scan_sensors(dir: &Path) -> Vec<Sensor> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let fname = e.file_name();
        let Some(fname) = fname.to_str() else {
            continue;
        };
        let Some(stem) = fname.strip_suffix("_input") else {
            continue;
        };
        let kind = [SensorKind::Temp, SensorKind::Fan, SensorKind::Power]
            .into_iter()
            .find(|k| stem.starts_with(k.prefix()));
        let Some(kind) = kind else { continue };
        // label 是選配的；沒有就用 sysfs 的節點名當 label。
        let label = util::read_trimmed(dir.join(format!("{stem}_label")))
            .unwrap_or_else(|_| stem.to_owned());
        out.push(Sensor {
            label,
            input: dir.join(fname),
            kind,
        });
    }
    out.sort_by(|a, b| a.input.cmp(&b.input));
    out
}

/// 找出第一顆 name 符合的晶片。
pub fn find_chip<'a>(chips: &'a [Chip], names: &[&str]) -> Option<&'a Chip> {
    chips.iter().find(|c| names.contains(&c.name.as_str()))
}

/// NVMe 硬碟的溫度：hwmon 的 `device` symlink 指回區塊裝置名稱。
pub fn nvme_temps(chips: &[Chip]) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for chip in chips.iter().filter(|c| c.name == "nvme") {
        // Composite 是整顆碟的代表溫度；沒有就取第一個溫度感測器。
        let temp = chip
            .read_labeled(|l| l.eq_ignore_ascii_case("Composite"))
            .or_else(|| {
                chip.sensors
                    .iter()
                    .find(|s| s.kind == SensorKind::Temp)
                    .and_then(|s| chip.read(s))
            });
        let Some(t) = temp else { continue };
        let dev = std::fs::read_link(chip.path.join("device"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "nvme".to_owned());
        out.push((dev, t));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_chip(root: &Path, name: &str) -> PathBuf {
        let d = root.join(name);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn scans_sensors_with_and_without_labels() {
        let tmp = tempfile::tempdir().unwrap();
        let d = fake_chip(tmp.path(), "hwmon9");
        fs::write(d.join("name"), "coretemp\n").unwrap();
        fs::write(d.join("temp1_input"), "61000\n").unwrap();
        fs::write(d.join("temp1_label"), "Package id 0\n").unwrap();
        fs::write(d.join("temp2_input"), "58000\n").unwrap(); // 沒有 label
        fs::write(d.join("fan1_input"), "1200\n").unwrap();
        fs::write(d.join("temp1_max"), "100000\n").unwrap(); // 不是 _input，要略過

        let sensors = scan_sensors(&d);
        assert_eq!(sensors.len(), 3, "只該收 _input 節點");
        let labels: Vec<_> = sensors.iter().map(|s| s.label.as_str()).collect();
        assert!(labels.contains(&"Package id 0"));
        assert!(labels.contains(&"temp2"), "沒有 label 時要退回節點名");
        assert!(labels.contains(&"fan1"));
    }

    #[test]
    fn scales_units_correctly() {
        assert_eq!(SensorKind::Temp.scale(61000), 61.0);
        assert_eq!(SensorKind::Fan.scale(1200), 1200.0);
        assert_eq!(SensorKind::Power.scale(15_000_000), 15.0);
    }

    #[test]
    fn read_returns_none_when_file_vanishes() {
        let tmp = tempfile::tempdir().unwrap();
        let d = fake_chip(tmp.path(), "hwmon0");
        fs::write(d.join("name"), "x").unwrap();
        let chip = Chip {
            name: "x".into(),
            path: d.clone(),
            sensors: vec![Sensor {
                label: "gone".into(),
                input: d.join("temp1_input"),
                kind: SensorKind::Temp,
            }],
        };
        assert_eq!(
            chip.read(&chip.sensors[0]),
            None,
            "檔案不存在要回 None 而不是 panic"
        );
    }

    #[test]
    fn read_returns_none_on_malformed_value() {
        let tmp = tempfile::tempdir().unwrap();
        let d = fake_chip(tmp.path(), "hwmon0");
        fs::write(d.join("temp1_input"), "not-a-number\n").unwrap();
        let chip = Chip {
            name: "x".into(),
            path: d.clone(),
            sensors: vec![Sensor {
                label: "bad".into(),
                input: d.join("temp1_input"),
                kind: SensorKind::Temp,
            }],
        };
        assert_eq!(chip.read(&chip.sensors[0]), None);
    }

    #[test]
    fn scan_missing_hwmon_dir_is_empty_not_panic() {
        assert!(scan_sensors(Path::new("/nonexistent/hwmon/path")).is_empty());
    }
}
