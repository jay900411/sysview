//! GPU collector：統一 NVIDIA / Intel / AMD 三家的模型。
//!
//! v1 用 `nvidia-smi` 的 CSV 輸出當主要 backend，那是錯的：每次啟動都要重新
//! 初始化驅動（實測 ~6 ms/次，比整個 sysview 還貴）。v2 改成直接走 **NVML**
//! （`nvidia-smi` 自己也是建在 NVML 上），一次呼叫是微秒等級。
//!
//! `nvidia-smi` 在 v2 只剩兩個角色：驅動壞掉時的**診斷**，以及 Explain 頁面上
//! 告訴使用者「同樣的數字用什麼原生指令可以看到」。

pub mod amd;
pub mod intel;
pub mod nvidia;

use std::time::Instant;

use crate::metrics::model::{Reading, Series, Unit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Intel,
    Amd,
}

impl Vendor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nvidia => "NVIDIA",
            Self::Intel => "Intel",
            Self::Amd => "AMD",
        }
    }
    /// PCI vendor id。
    pub fn from_pci(id: &str) -> Option<Self> {
        match id.trim() {
            "0x10de" => Some(Self::Nvidia),
            "0x8086" => Some(Self::Intel),
            "0x1002" => Some(Self::Amd),
            _ => None,
        }
    }
}

/// 在 GPU 上有 compute context 的行程。
#[derive(Debug, Clone)]
pub struct GpuProcess {
    pub pid: u32,
    pub used_memory: Option<u64>,
    /// 由 process collector 補上；NVML 本身不提供。
    pub name: Option<String>,
    pub user: Option<String>,
}

/// 一張 GPU 的統一模型。三家 backend 都填這個結構。
#[derive(Debug, Clone)]
pub struct GpuDevice {
    /// 穩定 id，用來對應歷史序列：nvidia 是 `nv0`，drm 是 `card1`。
    pub id: String,
    pub vendor: Vendor,
    pub name: String,
    /// 資料來自哪個介面，會顯示在 Explain 的 Source 欄。
    pub backend: &'static str,
    pub utilization: Reading,
    pub memory_used: Reading,
    pub memory_total: Reading,
    pub memory_utilization: Reading,
    pub temperature: Reading,
    pub power: Reading,
    pub power_limit: Reading,
    pub sm_clock: Reading,
    pub mem_clock: Reading,
    pub fan: Reading,
    pub pstate: Option<String>,
    /// 時脈範圍（Intel/AMD sysfs 有提供，NVIDIA 沒有）
    pub clock_range: Option<String>,
    pub driver_version: Option<String>,
    pub pcie_gen: Option<u32>,
    pub pcie_width: Option<u32>,
    pub processes: Vec<GpuProcess>,
    /// 一般使用者只看得到自己的 compute 行程，完整清單需要管理員權限。
    pub process_list_complete: bool,
}

impl GpuDevice {
    pub fn memory_percent(&self) -> Option<f64> {
        let (u, t) = (self.memory_used.get()?, self.memory_total.get()?);
        (t > 0.0).then(|| u / t * 100.0)
    }
}

/// 診斷訊息的嚴重程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagLevel {
    Info,
    Ok,
    Warn,
    Bad,
}

impl DiagLevel {
    /// 不能只靠顏色傳達狀態，所以每一級都有專屬符號。
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Info => "·",
            Self::Ok => "✓",
            Self::Warn => "!",
            Self::Bad => "✗",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagLine {
    pub level: DiagLevel,
    pub text: String,
}

/// GPU 子系統的整體狀態。
#[derive(Default)]
pub struct GpuState {
    pub devices: Vec<GpuDevice>,
    /// NVIDIA 驅動掛掉時的原因（NVML 初始化失敗訊息）。
    pub nvidia_error: Option<String>,
    /// 只有在真的壞掉時才會填 —— 診斷本身要 fork dpkg-query/dkms，很貴。
    pub diagnostics: Vec<DiagLine>,
    pub history: std::collections::HashMap<String, Series>,
    pub mem_history: std::collections::HashMap<String, Series>,
}

impl GpuState {
    pub fn by_id(&self, id: &str) -> Option<&GpuDevice> {
        self.devices.iter().find(|d| d.id == id)
    }
}

pub struct GpuCollector {
    nvidia: nvidia::NvidiaBackend,
    intel: intel::IntelBackend,
    amd: amd::AmdBackend,
    state: GpuState,
    history_len: usize,
    diagnosed: bool,
    enabled: bool,
}

impl GpuCollector {
    pub fn new(history_len: usize, enabled: bool) -> Self {
        Self {
            nvidia: nvidia::NvidiaBackend::new(enabled),
            intel: intel::IntelBackend::new(),
            amd: amd::AmdBackend::new(),
            state: GpuState::default(),
            history_len,
            diagnosed: false,
            enabled,
        }
    }

    pub fn state(&self) -> &GpuState {
        &self.state
    }

    /// GPU 上有沒有 compute 行程 —— 決定要不要用比較勤的間隔取樣。
    pub fn has_active_work(&self) -> bool {
        self.state
            .devices
            .iter()
            .any(|d| !d.processes.is_empty() || d.utilization.get().is_some_and(|u| u > 1.0))
    }

    pub fn update(&mut self, now: Instant) {
        if !self.enabled {
            return;
        }
        let mut devices = Vec::new();
        match self.nvidia.poll() {
            Ok(mut d) => {
                devices.append(&mut d);
                self.state.nvidia_error = None;
            }
            Err(e) => {
                let msg = e.to_string();
                // 只在第一次失敗、且系統上真的有 NVIDIA 卡時才做完整診斷。
                if !self.diagnosed && nvidia::has_nvidia_pci_device() {
                    self.state.diagnostics = nvidia::diagnose();
                    self.diagnosed = true;
                }
                // 只有在真的有卡卻連不上時才顯示錯誤；
                // 沒有 NVIDIA 的機器不該看到一堆紅字
                if nvidia::has_nvidia_pci_device() {
                    self.state.nvidia_error = Some(msg);
                }
            }
        }
        devices.append(&mut self.intel.poll(now));
        devices.append(&mut self.amd.poll());

        for d in &devices {
            self.state
                .history
                .entry(d.id.clone())
                .or_insert_with(|| Series::new(self.history_len))
                .push(d.utilization.or_zero());
            if let Some(p) = d.memory_percent() {
                self.state
                    .mem_history
                    .entry(d.id.clone())
                    .or_insert_with(|| Series::new(self.history_len))
                    .push(p);
            }
        }
        self.state.devices = devices;
    }

    /// 由 process collector 把 PID 對應到名稱與使用者。
    /// NVML 只給 PID，名稱要自己查 —— 而且一般使用者只查得到自己的行程。
    pub fn annotate_processes(&mut self, lookup: impl Fn(u32) -> Option<(String, String)>) {
        for d in &mut self.state.devices {
            for p in &mut d.processes {
                if let Some((name, user)) = lookup(p.pid) {
                    p.name = Some(name);
                    p.user = Some(user);
                }
            }
        }
    }
}

/// 給沒有值的欄位用的統一建構子，讓三家 backend 寫起來一致。
pub(crate) fn none(unit: Unit) -> Reading {
    Reading::unsupported(unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_pci_vendor_ids() {
        assert_eq!(Vendor::from_pci("0x10de"), Some(Vendor::Nvidia));
        assert_eq!(Vendor::from_pci("0x8086"), Some(Vendor::Intel));
        assert_eq!(Vendor::from_pci("0x1002"), Some(Vendor::Amd));
        assert_eq!(Vendor::from_pci("0xdead"), None);
        assert_eq!(
            Vendor::from_pci("  0x10de \n"),
            Some(Vendor::Nvidia),
            "應容忍空白"
        );
    }

    #[test]
    fn diag_levels_have_distinct_symbols() {
        // 色盲使用者必須能只靠符號分辨嚴重程度
        let syms: Vec<_> = [
            DiagLevel::Info,
            DiagLevel::Ok,
            DiagLevel::Warn,
            DiagLevel::Bad,
        ]
        .iter()
        .map(|l| l.symbol())
        .collect();
        let uniq: std::collections::HashSet<_> = syms.iter().collect();
        assert_eq!(
            uniq.len(),
            syms.len(),
            "每個等級都要有不同符號，不能只靠顏色"
        );
    }

    #[test]
    fn memory_percent_handles_missing_and_zero() {
        let mut d = GpuDevice {
            id: "nv0".into(),
            vendor: Vendor::Nvidia,
            name: "x".into(),
            backend: "NVML",
            utilization: none(Unit::Percent),
            memory_used: Reading::exact(50.0, Unit::Bytes),
            memory_total: Reading::exact(200.0, Unit::Bytes),
            memory_utilization: none(Unit::Percent),
            temperature: none(Unit::Celsius),
            power: none(Unit::Watts),
            power_limit: none(Unit::Watts),
            sm_clock: none(Unit::Megahertz),
            mem_clock: none(Unit::Megahertz),
            fan: none(Unit::Percent),
            pstate: None,
            clock_range: None,
            driver_version: None,
            pcie_gen: None,
            pcie_width: None,
            processes: vec![],
            process_list_complete: false,
        };
        assert_eq!(d.memory_percent(), Some(25.0));
        d.memory_total = Reading::exact(0.0, Unit::Bytes);
        assert_eq!(d.memory_percent(), None, "總量為 0 時不可除以零");
        d.memory_total = none(Unit::Bytes);
        assert_eq!(d.memory_percent(), None, "拿不到總量就不該有百分比");
    }
}
