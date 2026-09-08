//! NVIDIA backend：**NVML 是主要資料來源**。
//!
//! NVML 是 `nvidia-smi` 底下的那層函式庫，透過 `libloading` 在執行期
//! dlopen `libnvidia-ml.so`。因此：
//!
//! * 沒有 NVIDIA 驅動的機器一樣能編譯、能執行，只是這個 backend 回報不可用；
//! * 每次取樣是函式呼叫而不是 fork 一個行程，成本從毫秒降到微秒。
//!
//! `nvidia-smi` 只在驅動壞掉時被叫來做診斷，而且一律用 argv 形式呼叫，
//! 絕不組 shell 字串。

use std::process::Command;

use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::enums::device::UsedGpuMemory;
use nvml_wrapper::Nvml;

use crate::collectors::gpu::{none, DiagLevel, DiagLine, GpuDevice, GpuProcess, Vendor};
use crate::collectors::util;
use crate::metrics::model::{Reading, Unit};

pub struct NvidiaBackend {
    nvml: Option<Nvml>,
    /// NVML 初始化失敗的原因。延遲重試，不要每輪都嘗試 dlopen。
    init_error: Option<String>,
    driver_version: Option<String>,
    retry_countdown: u32,
    /// 完全停用這個 backend（`--no-gpu` / `gpu = false`）。
    disabled: bool,
}

impl NvidiaBackend {
    pub fn new(enabled: bool) -> Self {
        let mut s = Self {
            nvml: None,
            init_error: None,
            driver_version: None,
            retry_countdown: 0,
            disabled: !enabled,
        };
        // dlopen libnvidia-ml.so 會讓 RSS 增加約 15 MB（那是 NVIDIA 驅動自己的
        // 緩衝區，不是我們的）。所以只有在 PCI 上真的有 NVIDIA 顯示卡時才載入 ——
        // 沒有 NVIDIA 的機器完全不必付這個代價。
        if !enabled {
            s.init_error = Some("GPU backend disabled".into());
        } else if has_nvidia_pci_device() {
            s.try_init();
        } else {
            s.init_error = Some("系統上沒有 NVIDIA 顯示卡".into());
        }
        s
    }

    fn try_init(&mut self) {
        match Nvml::init() {
            Ok(n) => {
                self.driver_version = n.sys_driver_version().ok();
                self.nvml = Some(n);
                self.init_error = None;
            }
            Err(e) => {
                self.nvml = None;
                self.init_error = Some(friendly_nvml_error(&e));
            }
        }
    }

    pub fn poll(&mut self) -> Result<Vec<GpuDevice>, anyhow::Error> {
        if self.disabled {
            return Err(anyhow::anyhow!("GPU backend disabled (--no-gpu)"));
        }
        if self.nvml.is_none() && !has_nvidia_pci_device() {
            return Err(anyhow::anyhow!(
                "{}",
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "no NVIDIA device".into())
            ));
        }
        if self.nvml.is_none() {
            // 驅動可能是後來才載入的（例如使用者中途修好了），
            // 但每輪都 dlopen 很浪費，所以每 30 輪才重試一次。
            if self.retry_countdown == 0 {
                self.try_init();
                self.retry_countdown = 30;
            } else {
                self.retry_countdown -= 1;
            }
        }
        let Some(nvml) = self.nvml.as_ref() else {
            return Err(anyhow::anyhow!(
                "{}",
                self.init_error
                    .clone()
                    .unwrap_or_else(|| "NVML unavailable".into())
            ));
        };

        let count = match nvml.device_count() {
            Ok(c) => c,
            Err(e) => {
                // 驅動在執行期間掛掉（例如被 rmmod）
                self.nvml = None;
                self.init_error = Some(friendly_nvml_error(&e));
                return Err(anyhow::anyhow!("{}", friendly_nvml_error(&e)));
            }
        };

        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            // Device 借用 Nvml，所以每輪重新取得 handle（這只是查表，很便宜），
            // 避免自我參照結構。
            let Ok(dev) = nvml.device_by_index(i) else {
                continue;
            };
            out.push(read_device(&dev, i, self.driver_version.clone()));
        }
        Ok(out)
    }
}

impl Default for NvidiaBackend {
    fn default() -> Self {
        Self::new(true)
    }
}

fn read_device(dev: &nvml_wrapper::Device<'_>, index: u32, driver: Option<String>) -> GpuDevice {
    let util = dev.utilization_rates().ok();
    let mem = dev.memory_info().ok();

    let processes = dev
        .running_compute_processes()
        .unwrap_or_default()
        .into_iter()
        .map(|p| GpuProcess {
            pid: p.pid,
            used_memory: match p.used_gpu_memory {
                UsedGpuMemory::Used(b) => Some(b),
                UsedGpuMemory::Unavailable => None,
            },
            name: None,
            user: None,
        })
        .collect();

    GpuDevice {
        id: format!("nv{index}"),
        vendor: Vendor::Nvidia,
        name: dev.name().unwrap_or_else(|_| format!("NVIDIA GPU {index}")),
        backend: "NVML (libnvidia-ml.so)",
        utilization: match &util {
            Some(u) => Reading::exact(u.gpu as f64, Unit::Percent).with_id("gpu.util"),
            None => none(Unit::Percent).with_id("gpu.util"),
        },
        memory_utilization: match &util {
            Some(u) => Reading::exact(u.memory as f64, Unit::Percent).with_id("gpu.vram"),
            None => none(Unit::Percent),
        },
        memory_used: match &mem {
            Some(m) => Reading::exact(m.used as f64, Unit::Bytes).with_id("gpu.vram"),
            None => none(Unit::Bytes).with_id("gpu.vram"),
        },
        memory_total: match &mem {
            Some(m) => Reading::exact(m.total as f64, Unit::Bytes).with_id("gpu.vram"),
            None => none(Unit::Bytes),
        },
        temperature: opt_reading(
            dev.temperature(TemperatureSensor::Gpu)
                .ok()
                .map(|v| v as f64),
            Unit::Celsius,
            "gpu.temp",
        ),
        // NVML 回傳毫瓦
        power: opt_reading(
            dev.power_usage().ok().map(|v| v as f64 / 1000.0),
            Unit::Watts,
            "gpu.power",
        ),
        power_limit: opt_reading(
            dev.enforced_power_limit().ok().map(|v| v as f64 / 1000.0),
            Unit::Watts,
            "gpu.power",
        ),
        sm_clock: opt_reading(
            dev.clock_info(Clock::SM).ok().map(|v| v as f64),
            Unit::Megahertz,
            "gpu.clock",
        ),
        mem_clock: opt_reading(
            dev.clock_info(Clock::Memory).ok().map(|v| v as f64),
            Unit::Megahertz,
            "gpu.clock",
        ),
        fan: opt_reading(
            dev.fan_speed(0).ok().map(|v| v as f64),
            Unit::Percent,
            "gpu.fan",
        ),
        pstate: dev.performance_state().ok().map(pstate_name),
        clock_range: None,
        driver_version: driver,
        pcie_gen: dev.current_pcie_link_gen().ok(),
        pcie_width: dev.current_pcie_link_width().ok(),
        processes,
        // NVML 只列出呼叫者有權限看到的行程；一般使用者看不到別人的。
        process_list_complete: util::effective_uid() == 0,
    }
}

/// NVML 的 PerformanceState enum 的 Debug 名稱是 "Zero"/"Eight"，
/// 但所有 NVIDIA 文件與工具講的都是 P0/P8，照著顯示才不會讓使用者困惑。
fn pstate_name(p: nvml_wrapper::enum_wrappers::device::PerformanceState) -> String {
    use nvml_wrapper::enum_wrappers::device::PerformanceState as P;
    let n = match p {
        P::Zero => "P0",
        P::One => "P1",
        P::Two => "P2",
        P::Three => "P3",
        P::Four => "P4",
        P::Five => "P5",
        P::Six => "P6",
        P::Seven => "P7",
        P::Eight => "P8",
        P::Nine => "P9",
        P::Ten => "P10",
        P::Eleven => "P11",
        P::Twelve => "P12",
        P::Thirteen => "P13",
        P::Fourteen => "P14",
        P::Fifteen => "P15",
        P::Unknown => "unknown",
    };
    match p {
        P::Zero => format!("{n} (最高效能)"),
        P::Eight | P::Nine | P::Ten | P::Eleven | P::Twelve => format!("{n} (省電閒置)"),
        _ => n.to_owned(),
    }
}

fn opt_reading(v: Option<f64>, unit: Unit, id: &'static str) -> Reading {
    match v {
        Some(v) => Reading::exact(v, unit).with_id(id),
        // 筆電卡、虛擬化環境、部分型號不支援某些感測器 —— 這是正常的
        None => none(unit).with_id(id),
    }
}

/// 把 NVML 的錯誤翻成使用者看得懂的一句話。
fn friendly_nvml_error(e: &nvml_wrapper::error::NvmlError) -> String {
    use nvml_wrapper::error::NvmlError as E;
    match e {
        E::LibloadingError(_) => "找不到 libnvidia-ml.so —— 系統上沒有安裝 NVIDIA 驅動".to_owned(),
        E::DriverNotLoaded => "NVIDIA 核心模組沒有載入".to_owned(),
        E::NoPermission => "沒有權限存取 NVIDIA 裝置節點".to_owned(),
        E::Uninitialized => "NVML 尚未初始化".to_owned(),
        other => format!("NVML: {other}"),
    }
}

/// PCI 匯流排上有沒有 NVIDIA 顯示卡。
///
/// 直接讀 sysfs 而不是 fork `lspci` —— 這在每次取樣失敗時都會被呼叫。
pub fn has_nvidia_pci_device() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return false;
    };
    for e in dir.flatten() {
        let p = e.path();
        // class 0x03xxxx = Display controller
        let is_display = util::read_trimmed(p.join("class")).is_ok_and(|c| c.starts_with("0x03"));
        if !is_display {
            continue;
        }
        if util::read_trimmed(p.join("vendor")).as_deref() == Ok("0x10de") {
            return true;
        }
    }
    false
}

/// NVML 起不來時，把「為什麼」查出來。
///
/// **這個函式很貴**（會 fork `dpkg-query`、`dkms`、`modinfo`），
/// 所以只在真的偵測到有 NVIDIA 卡但驅動不通時呼叫一次，不進入取樣迴圈。
///
/// 所有外部指令都用 argv 形式呼叫，沒有任何一處組 shell 字串。
pub fn diagnose() -> Vec<DiagLine> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<DiagLine>, level, text: String| {
        out.push(DiagLine { level, text });
    };

    // 1. PCI 上有哪張卡
    for name in nvidia_pci_names() {
        push(
            &mut out,
            DiagLevel::Info,
            format!("PCI 上偵測到 NVIDIA: {name}"),
        );
    }

    // 2. kernel module 有沒有載入
    let loaded = util::read_string("/proc/modules")
        .map(|m| m.lines().any(|l| l.starts_with("nvidia ")))
        .unwrap_or(false);
    push(
        &mut out,
        if loaded {
            DiagLevel::Ok
        } else {
            DiagLevel::Bad
        },
        format!(
            "kernel module `nvidia` {}",
            if loaded { "已載入" } else { "未載入" }
        ),
    );

    // 3. 裝置節點在不在
    let devs: Vec<String> = std::fs::read_dir("/dev")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("nvidia").then_some(n)
        })
        .take(6)
        .collect();
    push(
        &mut out,
        if devs.is_empty() {
            DiagLevel::Bad
        } else {
            DiagLevel::Ok
        },
        if devs.is_empty() {
            "/dev/nvidia* 裝置節點: 不存在".to_owned()
        } else {
            format!("/dev/nvidia* 裝置節點: {}", devs.join(", "))
        },
    );

    // 4. 裝了哪些驅動套件（多版本並存會互相打架）
    if let Some(pkgs) = run(&[
        "dpkg-query",
        "-W",
        "-f=${Package} ${Version}\n",
        "nvidia-dkms-*",
        "nvidia-driver-*",
    ]) {
        let lines: Vec<&str> = pkgs
            .lines()
            .filter(|l| l.split_ascii_whitespace().count() >= 2)
            .collect();
        if !lines.is_empty() {
            push(
                &mut out,
                DiagLevel::Info,
                format!("已安裝套件: {}", lines.join("; ")),
            );
            let mut versions: Vec<&str> = lines
                .iter()
                .filter_map(|l| l.strip_prefix("nvidia-dkms-"))
                .filter_map(|r| r.split(|c: char| !c.is_ascii_digit()).next())
                .filter(|s| !s.is_empty())
                .collect();
            versions.sort_unstable();
            versions.dedup();
            if versions.len() > 1 {
                push(
                    &mut out,
                    DiagLevel::Bad,
                    format!(
                        "同時裝了多個 nvidia-dkms 版本 ({}) — 會互相打架",
                        versions.join(", ")
                    ),
                );
            }
        }
    }

    // 5. DKMS 模組是替哪個核心編的 —— 核心升級後沒重建是最常見的病因
    let current_kernel = kernel_release();
    if let Some(status) = run(&["dkms", "status"]) {
        let status = status.trim();
        if status.is_empty() {
            push(
                &mut out,
                DiagLevel::Bad,
                "dkms status 是空的 — 沒有任何模組替目前核心編譯過".into(),
            );
        } else {
            let mut kernels: Vec<String> = Vec::new();
            for line in status.lines().take(6) {
                push(&mut out, DiagLevel::Info, format!("dkms: {}", line.trim()));
                // 格式：nvidia/550.90.07, 6.8.0-40-generic, x86_64: installed
                let parts: Vec<&str> = line.split(", ").collect();
                if parts.len() >= 2 {
                    let k = parts[1].trim().to_owned();
                    if !k.is_empty() && !kernels.contains(&k) {
                        kernels.push(k);
                    }
                }
            }
            if !kernels.is_empty() && !kernels.iter().any(|k| k == &current_kernel) {
                push(
                    &mut out,
                    DiagLevel::Bad,
                    format!(
                        "模組只替 {} 編過，但目前跑的是 {current_kernel} — 核心升級後沒重建",
                        kernels.join(" / ")
                    ),
                );
            }
        }
    }
    push(
        &mut out,
        DiagLevel::Info,
        format!("目前核心: {current_kernel}"),
    );

    // 6. Secure Boot 會擋未簽章的模組
    if let Some(sb) = run(&["mokutil", "--sb-state"]) {
        if let Some(first) = sb.lines().next() {
            let on = first.to_ascii_lowercase().contains("enabled");
            push(
                &mut out,
                if on { DiagLevel::Warn } else { DiagLevel::Info },
                format!("Secure Boot: {}", first.trim()),
            );
        }
    }
    out
}

/// 修復建議。UI 會原樣顯示，讓使用者自己決定要不要執行。
pub fn repair_hints() -> &'static [&'static str] {
    &[
        "1. 只留一個版本：sudo apt purge 'nvidia-dkms-<舊版本>*'",
        "2. 重建模組：    sudo dkms autoinstall -k $(uname -r)",
        "3. 或整包重裝：  sudo apt install --reinstall nvidia-driver-<版本>",
        "4. 載入並驗證：  sudo modprobe nvidia && nvidia-smi",
        "5. 若 Secure Boot 開著，模組需簽章，重開機後跑 MOK 註冊流程",
    ]
}

fn nvidia_pci_names() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return out;
    };
    for e in dir.flatten() {
        let p = e.path();
        if !util::read_trimmed(p.join("class")).is_ok_and(|c| c.starts_with("0x03")) {
            continue;
        }
        if util::read_trimmed(p.join("vendor")).as_deref() != Ok("0x10de") {
            continue;
        }
        let dev = util::read_trimmed(p.join("device")).unwrap_or_default();
        let slot = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(format!("{slot} (device {dev})"));
    }
    out
}

fn kernel_release() -> String {
    util::read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_else(|_| "unknown".into())
}

/// 執行外部診斷指令。
///
/// **安全性**：一律用 argv 形式，第一個元素是程式名、其餘是參數。
/// 沒有 shell、沒有字串串接、沒有使用者輸入進到這裡。
/// 指令不存在或失敗都回 `None`，不當成錯誤。
fn run(argv: &[&str]) -> Option<String> {
    let (prog, args) = argv.split_first()?;
    let out = Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_hints_are_non_empty_and_mention_sudo() {
        let h = repair_hints();
        assert!(!h.is_empty());
        assert!(
            h.iter().any(|s| s.contains("sudo")),
            "修復步驟需要 root，應明講"
        );
    }

    #[test]
    fn diagnose_does_not_panic_without_nvidia() {
        // 在任何機器上都必須安全執行完（沒有 dkms/mokutil 也一樣）
        let d = diagnose();
        assert!(d.iter().all(|l| !l.text.is_empty()));
    }

    #[test]
    fn run_returns_none_for_missing_binary() {
        assert!(run(&["definitely-not-a-real-binary-xyz"]).is_none());
    }

    #[test]
    fn run_never_uses_a_shell() {
        // 若不小心走 shell，這個「參數」會被展開成通配符或指令替換
        let out = run(&["echo", "$(id -u)", "*"]);
        if let Some(o) = out {
            assert!(
                o.contains("$(id -u)"),
                "參數必須原樣傳遞，不得經過 shell 展開"
            );
            assert!(o.contains('*'), "通配符不得被展開");
        }
    }

    #[test]
    fn kernel_release_is_readable() {
        let k = kernel_release();
        assert!(!k.is_empty());
    }
}
