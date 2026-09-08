//! 與 C++ v1 的 parity 測試。
//!
//! v1 是這次重寫的 **behavioral oracle**。兩者在同一台機器上同時取樣，
//! 數量級與語意必須一致。取樣視窗不同會造成小差異，那是預期內的。

use std::path::PathBuf;
use std::process::Command;

fn legacy_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("legacy/cpp")
}

fn legacy_binary() -> Option<PathBuf> {
    let p = legacy_root().join("sysview");
    p.is_file().then_some(p)
}

fn legacy_snapshot() -> Option<String> {
    let out = Command::new(legacy_binary()?)
        .arg("--snapshot")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 從 legacy 的文字輸出裡撈出一個數字。
fn grab(text: &str, prefix: &str, needle: &str) -> Option<f64> {
    let line = text.lines().find(|l| l.trim_start().starts_with(prefix))?;
    let idx = line.find(needle)?;
    let rest = &line[idx + needle.len()..];
    let num: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok()
}

fn rust_state() -> sysview::collectors::SystemState {
    let mut s = sysview::collectors::SystemState::new(
        60,
        sysview::collectors::Intervals::from_base(std::time::Duration::from_millis(300)),
    );
    s.sample_all(std::time::Instant::now());
    std::thread::sleep(std::time::Duration::from_millis(300));
    s.sample_all(std::time::Instant::now());
    s
}

/// oracle 缺席時，「刻意拿掉」與「忘了編」必須分得出來。
///
/// 這些比對在找不到 oracle 時會直接 return，所以測試照樣是綠的。
/// 沒有這道檢查的話，忘記 `make legacy` 會靜靜地少測四項，
/// 而畫面上看起來一切正常 —— 那比測試失敗還糟。
#[test]
fn legacy_oracle_is_available() {
    if legacy_binary().is_some() {
        return;
    }
    // 整個 legacy/cpp 都不在 → 有人刻意拿掉了 v1，那就不需要 oracle
    if !legacy_root().join("sysview.cpp").is_file() {
        eprintln!("legacy/cpp 不存在，parity 比對已停用（這是刻意的）");
        return;
    }
    // 沒有 C++ 編譯器 → 這台機器**編不出來**，不是「忘了編」。
    //
    // v1 的執行檔是建置產物，不進版控（它綁 glibc 版本，別台機器拿到也
    // 不能跑）。所以剛 clone 下來、或在只裝了 Rust 的機器上，這裡本來就
    // 沒有 oracle —— 那不該讓整個測試套件變紅。CI 會先跑 `make legacy`，
    // 比對才真的會執行。
    let has_cxx = ["c++", "g++", "clang++"].iter().any(|cc| {
        std::process::Command::new(cc)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    });
    if !has_cxx {
        eprintln!(
            "這台機器沒有 C++ 編譯器，編不出 v1 oracle，parity 比對已跳過。\n\
             （sysview 本身不需要 C++，只有這個回歸比對基準需要。）"
        );
        return;
    }
    // 編譯器在、原始碼在、執行檔不在 → 是忘了編
    panic!(
        "legacy/cpp/sysview.cpp 在，但沒有編出執行檔。\n\
         這樣有 4 個 parity 比對會靜靜地跳過。請先執行 `make legacy`，\n\
         或者如果不再需要 v1 當比對基準，把整個 legacy/cpp 拿掉。\n\
         （v1 的執行檔是建置產物，刻意不進版控。）"
    );
}

#[test]
fn cpu_core_count_matches_legacy() {
    let Some(text) = legacy_snapshot() else {
        return;
    };
    let s = rust_state();
    let legacy_threads = grab(&text, "24 執行緒", "").or_else(|| {
        text.lines()
            .find(|l| l.contains("執行緒 ·"))
            .and_then(|l| l.split_whitespace().next()?.parse().ok())
    });
    if let Some(n) = legacy_threads {
        assert_eq!(
            s.cpu.state().logical as f64,
            n,
            "邏輯核心數必須與 v1 完全一致"
        );
    }
}

#[test]
fn memory_total_matches_legacy_exactly() {
    let Some(text) = legacy_snapshot() else {
        return;
    };
    let s = rust_state();
    // v1 印的是 "5.4 GB / 62.5 GB"
    let line = text.lines().find(|l| l.trim_start().starts_with("MEM"));
    let Some(line) = line else { return };
    let Some(total_str) = line.split('/').nth(1) else {
        return;
    };
    let total: f64 = total_str
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    if total > 0.0 {
        let rust_gb = s.memory.state().total() as f64 / 1024.0 / 1024.0 / 1024.0;
        assert!(
            (rust_gb - total).abs() < 0.5,
            "記憶體總量不一致：v1 {total} GB vs v2 {rust_gb:.1} GB"
        );
    }
}

#[test]
fn mount_points_match_legacy() {
    let Some(text) = legacy_snapshot() else {
        return;
    };
    let s = rust_state();
    let legacy_mounts: Vec<String> = text
        .lines()
        .filter(|l| l.trim_start().starts_with('/'))
        .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
        .collect();
    if legacy_mounts.is_empty() {
        return;
    }
    let rust_mounts: Vec<&str> = s
        .disk
        .state()
        .mounts
        .iter()
        .map(|m| m.mount_point.as_str())
        .collect();
    for m in &legacy_mounts {
        assert!(
            rust_mounts.contains(&m.as_str()),
            "v1 有掛載點 {m} 但 v2 沒有；v2 的清單：{rust_mounts:?}"
        );
    }
    assert_eq!(
        rust_mounts.len(),
        legacy_mounts.len(),
        "掛載點數量不一致（去重邏輯應相同）"
    );
}

#[test]
fn disk_usage_matches_df_semantics_like_legacy() {
    // v1 已經改成 df 的 used/(used+avail) 定義，v2 必須維持
    let s = rust_state();
    let out = Command::new("df")
        .args(["-P", "--output=target,pcent"])
        .output();
    let Ok(out) = out else { return };
    if !out.status.success() {
        return;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().skip(1) {
        let mut it = line.split_whitespace();
        let (Some(target), Some(pcent)) = (it.next(), it.next()) else {
            continue;
        };
        let Ok(df_pct) = pcent.trim_end_matches('%').parse::<f64>() else {
            continue;
        };
        let Some(m) = s
            .disk
            .state()
            .mounts
            .iter()
            .find(|m| m.mount_point == target)
        else {
            continue;
        };
        let ours = m.usage.get().unwrap_or(-1.0);
        assert!(
            (ours - df_pct).abs() <= 1.0,
            "{target} 的使用率與 df 不符：df {df_pct}% vs sysview {ours:.1}%"
        );
    }
}

#[test]
fn process_count_is_within_sampling_noise_of_ps() {
    let s = rust_state();
    let out = Command::new("ps").args(["-e", "--no-headers"]).output();
    let Ok(out) = out else { return };
    let ps_count = String::from_utf8_lossy(&out.stdout).lines().count();
    let ours = s.process.state().total;
    let diff = (ours as i64 - ps_count as i64).abs();
    assert!(
        diff < 40,
        "行程數差異過大：ps {ps_count} vs sysview {ours}（取樣時間差應該只造成個位數差異）"
    );
}

#[test]
fn nvidia_data_matches_nvidia_smi_when_available() {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(out) = out else { return };
    if !out.status.success() {
        eprintln!("nvidia-smi 不可用，略過");
        return;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(line) = text.lines().next() else {
        return;
    };
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    if fields.len() < 3 {
        return;
    }

    let s = rust_state();
    let dev = s
        .gpu
        .state()
        .devices
        .iter()
        .find(|d| d.vendor == sysview::collectors::gpu::Vendor::Nvidia);
    let Some(dev) = dev else {
        panic!("nvidia-smi 看得到 GPU，但 NVML backend 沒有回報任何裝置");
    };

    assert_eq!(dev.name, fields[0], "GPU 型號應與 nvidia-smi 一致");
    // memory.total 是 MiB
    if let (Ok(smi_mib), Some(ours)) = (fields[1].parse::<f64>(), dev.memory_total.get()) {
        let ours_mib = ours / 1024.0 / 1024.0;
        assert!(
            (ours_mib - smi_mib).abs() < 2.0,
            "VRAM 總量不一致：nvidia-smi {smi_mib} MiB vs NVML {ours_mib:.0} MiB"
        );
    }
    if let (Ok(smi_temp), Some(ours)) = (fields[2].parse::<f64>(), dev.temperature.get()) {
        assert!(
            (ours - smi_temp).abs() <= 5.0,
            "溫度差異過大：nvidia-smi {smi_temp}°C vs NVML {ours}°C"
        );
    }
    assert_eq!(
        dev.backend, "NVML (libnvidia-ml.so)",
        "NVIDIA 的主要 backend 必須是 NVML 而不是 nvidia-smi"
    );
}

#[test]
fn load_average_matches_proc_loadavg() {
    let s = rust_state();
    let text = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let first: f64 = text
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1.0);
    if first >= 0.0 {
        assert!(
            (s.cpu.state().load[0] - first).abs() < 0.5,
            "load average 應直接反映 /proc/loadavg"
        );
    }
}

#[test]
fn json_output_matches_internal_state() {
    let v = sysview::snapshot::json(60, std::time::Duration::from_millis(200), true);
    assert!(
        v["cpu"]["logical"].as_u64().unwrap_or(0) > 0,
        "JSON 應含邏輯核心數"
    );
    assert!(
        v["memory"]["total"].as_u64().unwrap_or(0) > 0,
        "JSON 應含記憶體總量"
    );
    assert!(v["storage"]["mounts"].is_array());
    assert!(v["network"]["interfaces"].is_array());
    assert!(v["processes"]["total"].as_u64().unwrap_or(0) > 0);
}
