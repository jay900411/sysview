//! 非互動輸出：純文字快照與 JSON。
//!
//! 沒有 TTY 時（`sysview > log.txt`、cron、pipe）自動切到這個模式。
//!
//! # 權限
//!
//! JSON 輸出**一樣遵守呼叫者的權限**。加上 `--json` 不會、也不可能繞過
//! 任何檔案權限 —— 資料來源與互動模式完全相同。管理員資料需要另外
//! 透過 sudo helper 取得，不會因為換了輸出格式就自動出現。

use std::time::Instant;

use serde_json::json;

use crate::collectors::process::SortKey;
use crate::collectors::{Intervals, SystemState};
use crate::ui::format;

/// 取兩次樣（中間隔 `interval`），才算得出速率。
fn sample_twice_opt(history: usize, interval: std::time::Duration, gpu: bool) -> SystemState {
    let mut state = SystemState::with_options(history, Intervals::from_base(interval), gpu);
    state.sample_all(Instant::now());
    std::thread::sleep(interval);
    state.sample_all(Instant::now());
    state
}

fn bar(pct: f64, width: usize) -> String {
    let pct = if pct.is_finite() {
        pct.clamp(0.0, 100.0)
    } else {
        0.0
    };
    let n = (pct / 100.0 * width as f64) as usize;
    format!("[{}{}]", "#".repeat(n), ".".repeat(width - n))
}

/// 純文字快照。刻意只用 ASCII 畫圖，方便 grep 與貼進 issue。
pub fn text(history: usize, interval: std::time::Duration, gpu: bool) -> String {
    let s = sample_twice_opt(history, interval, gpu);
    let mut o = String::new();
    let mut line = |x: String| {
        o.push_str(&x);
        o.push('\n');
    };

    let c = s.cpu.state();
    let m = s.memory.state();
    line("=".repeat(78));
    line(format!(
        " sysview {}   {}   up {}",
        crate::VERSION,
        crate::ui::hostname(),
        format::duration(c.uptime_secs)
    ));
    line("=".repeat(78));
    line(String::new());

    // CPU
    line(format!(
        " CPU  {}  {:>6}   {}",
        bar(c.usage.or_zero(), 28),
        format::reading(&c.usage),
        format::truncate(&c.model, 34)
    ));
    line(format!(
        "      {} threads · {} · {} · load {:.2} {:.2} {:.2} · iowait {}",
        c.logical,
        format::reading(&c.freq_avg()),
        format::reading(&c.temp),
        c.load[0],
        c.load[1],
        c.load[2],
        format::reading(&c.iowait)
    ));
    let mut row = String::from("      ");
    for (i, core) in c.cores.iter().enumerate() {
        row.push_str(&format!("{i:>2}:{:>4.0}%  ", core.usage.or_zero()));
        if (i + 1) % 8 == 0 {
            line(std::mem::replace(&mut row, String::from("      ")));
        }
    }
    if !row.trim().is_empty() {
        line(row);
    }
    line(String::new());

    // Memory
    line(format!(
        " MEM  {}  {:>6}   {} / {}",
        bar(m.used_percent().or_zero(), 28),
        format::reading(&m.used_percent()),
        format::bytes(m.used() as f64),
        format::bytes(m.total() as f64)
    ));
    if m.swap_total() > 0 {
        line(format!(
            " SWAP {}  {:>6}   {} / {}",
            bar(m.swap_percent().or_zero(), 28),
            format::reading(&m.swap_percent()),
            format::bytes(m.swap_used() as f64),
            format::bytes(m.swap_total() as f64)
        ));
    }
    line(format!(
        "      cache {} · buffers {} · available {}",
        format::bytes(m.reclaimable_cache() as f64),
        format::bytes(m.buffers() as f64),
        format::bytes(m.available() as f64)
    ));
    line(String::new());

    // GPU
    let g = s.gpu.state();
    if let Some(err) = &g.nvidia_error {
        line(format!(" GPU  [NVIDIA unavailable] {err}"));
        for d in &g.diagnostics {
            line(format!("      {} {}", d.level.symbol(), d.text));
        }
    }
    for d in &g.devices {
        line(format!(
            " GPU  {}  {:>7}   {}",
            bar(d.utilization.or_zero(), 28),
            format::reading(&d.utilization),
            format::truncate(&d.name, 34)
        ));
        // 只列出真的有值的欄位，不要用一整排 "unsupported" 洗版
        let mut bits = vec![d.backend.to_owned()];
        if d.memory_total.get().is_some() {
            bits.push(format!(
                "VRAM {} / {}",
                format::reading(&d.memory_used),
                format::reading(&d.memory_total)
            ));
        }
        for (label, r) in [
            ("temp", &d.temperature),
            ("power", &d.power),
            ("clock", &d.sm_clock),
        ] {
            if r.get().is_some() {
                bits.push(format!("{label} {}", format::reading(r)));
            }
        }
        if d.utilization.quality.is_estimated() {
            bits.push("utilization is estimated".to_owned());
        }
        line(format!("      {}", bits.join(" · ")));
        for p in &d.processes {
            line(format!(
                "      pid {:<8} {:>10}  {}",
                p.pid,
                p.used_memory
                    .map(|v| format::bytes(v as f64))
                    .unwrap_or_else(|| "n/a".into()),
                p.name.clone().unwrap_or_else(|| "?".into())
            ));
        }
    }
    if g.devices.is_empty() && g.nvidia_error.is_none() {
        line(" GPU  none detected".into());
    }
    line(String::new());

    // Storage
    let d = s.disk.state();
    line(format!(
        " DISK  read {}  write {}",
        format::reading(&d.total_read),
        format::reading(&d.total_write)
    ));
    for mnt in &d.mounts {
        line(format!(
            "      {:<20} {} {:>4.0}%  {:>10} free / {:>10}",
            format::truncate(&mnt.mount_point, 20),
            bar(mnt.usage.or_zero(), 20),
            mnt.usage.or_zero(),
            format::bytes(mnt.available as f64),
            format::bytes(mnt.total as f64)
        ));
    }
    line(String::new());

    // Network
    let n = s.network.state();
    line(format!(
        " NET   down {}   up {}",
        format::reading(&n.total_rx),
        format::reading(&n.total_tx)
    ));
    for it in n.interfaces.iter().filter(|i| !i.virtual_iface && i.up) {
        line(format!(
            "      {:<12} down {:>12}  up {:>12}   {}",
            it.name,
            format::reading(&it.rx_rate),
            format::reading(&it.tx_rate),
            it.addrs.join(", ")
        ));
    }
    line(String::new());

    // Processes
    let p = s.process.state();
    line(format!(
        " PROC  {} processes / {} threads",
        p.total, p.threads
    ));
    line(format!(
        "      {:>7} {:<12} {:>7} {:>11}  {}",
        "PID", "USER", "CPU%", "RSS", "COMMAND"
    ));
    for q in p.top(SortKey::Cpu, 12, "") {
        line(format!(
            "      {:>7} {:<12} {:>7.1} {:>11}  {}",
            q.pid,
            format::truncate(&q.user, 12),
            q.cpu_percent,
            format::bytes(q.rss as f64),
            format::truncate(&q.cmdline, 44)
        ));
    }
    line(String::new());
    o
}

/// 機器可讀的 JSON 輸出，適合 `sysview --json | jq`。
pub fn json(history: usize, interval: std::time::Duration, gpu: bool) -> serde_json::Value {
    let s = sample_twice_opt(history, interval, gpu);
    let c = s.cpu.state();
    let m = s.memory.state();
    let g = s.gpu.state();
    let d = s.disk.state();
    let n = s.network.state();
    let p = s.process.state();

    let r = |x: &crate::metrics::model::Reading| {
        json!({
            "value": x.value,
            "unit": x.unit,
            "quality": x.quality,
        })
    };

    json!({
        "sysview_version": crate::VERSION,
        "schema": 1,
        // 明講權限邊界，避免使用者以為 --json 會拿到更多東西
        "permissions": {
            "euid": crate::collectors::util::effective_uid(),
            "note": "所有資料都受呼叫者的檔案權限限制；--json 不會提升權限。\
                     管理員資料需另外透過 sudo sysview-priv 取得。"
        },
        "host": {
            "hostname": crate::ui::hostname(),
            "uptime_seconds": c.uptime_secs,
            "environment": s.cgroup.environment.label(),
        },
        "cgroup": {
            "cpu_limit_cores": s.cgroup.cpu_limit_cores,
            "memory_limit": s.cgroup.memory_limit,
            "memory_current": s.cgroup.memory_current,
        },
        "cpu": {
            "model": c.model,
            "logical": c.logical,
            "physical": c.physical,
            "usage": r(&c.usage),
            "frequency": r(&c.freq_avg()),
            "temperature": r(&c.temp),
            "load": c.load,
            "iowait": r(&c.iowait),
            "steal": r(&c.steal),
            "context_switches": r(&c.ctxt_rate),
            "cores": c.cores.iter().map(|x| json!({
                "index": x.index, "usage": r(&x.usage), "frequency": r(&x.freq_mhz),
            })).collect::<Vec<_>>(),
        },
        "memory": {
            "total": m.total(), "used": m.used(), "available": m.available(),
            "cache": m.reclaimable_cache(), "buffers": m.buffers(),
            "swap_total": m.swap_total(), "swap_used": m.swap_used(),
            "used_percent": r(&m.used_percent()),
        },
        "gpu": {
            "nvidia_error": g.nvidia_error,
            "devices": g.devices.iter().map(|x| json!({
                "id": x.id, "vendor": x.vendor.label(), "name": x.name,
                "backend": x.backend,
                "utilization": r(&x.utilization),
                "memory_used": r(&x.memory_used),
                "memory_total": r(&x.memory_total),
                "temperature": r(&x.temperature),
                "power": r(&x.power),
                "sm_clock": r(&x.sm_clock),
                "processes": x.processes.iter().map(|p| json!({
                    "pid": p.pid, "used_memory": p.used_memory, "name": p.name,
                })).collect::<Vec<_>>(),
                "process_list_complete": x.process_list_complete,
            })).collect::<Vec<_>>(),
        },
        "storage": {
            "read": r(&d.total_read), "write": r(&d.total_write),
            "mounts": d.mounts.iter().map(|x| json!({
                "mount_point": x.mount_point, "device": x.device, "fs_type": x.fs_type,
                "total": x.total, "used": x.used, "available": x.available,
                "usage_percent": r(&x.usage), "inode_percent": r(&x.inode_usage),
            })).collect::<Vec<_>>(),
            "devices": d.devices.iter().map(|x| json!({
                "name": x.name, "model": x.model, "kind": x.kind(), "size": x.size,
                "read": r(&x.read_rate), "write": r(&x.write_rate),
                "iops": r(&x.iops), "busy": r(&x.util), "temperature": r(&x.temp),
            })).collect::<Vec<_>>(),
        },
        "network": {
            "rx": r(&n.total_rx), "tx": r(&n.total_tx),
            "interfaces": n.interfaces.iter().map(|x| json!({
                "name": x.name, "up": x.up, "virtual": x.virtual_iface,
                "mac": x.mac, "addresses": x.addrs, "speed_mbit": x.speed_mbit,
                "rx": r(&x.rx_rate), "tx": r(&x.tx_rate),
                "rx_total": x.rx_total, "tx_total": x.tx_total,
                "errors": x.errors, "drops": x.drops,
            })).collect::<Vec<_>>(),
        },
        "processes": {
            "total": p.total,
            "threads": p.threads,
            "top": p.top(SortKey::Cpu, 20, "").iter().map(|q| json!({
                "pid": q.pid, "ppid": q.ppid, "user": q.user, "state": q.state.to_string(),
                "cpu_percent": q.cpu_percent, "rss": q.rss, "vsize": q.vsize,
                "threads": q.threads, "cpu_time": q.cpu_time_secs, "command": q.cmdline,
            })).collect::<Vec<_>>(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bar_never_overflows() {
        for p in [-10.0, 0.0, 50.0, 100.0, 200.0, f64::NAN] {
            let b = bar(p, 20);
            assert_eq!(b.len(), 22, "{p} 產生了錯誤長度的長條：{b}");
        }
    }

    #[test]
    fn text_snapshot_has_all_sections() {
        let out = text(30, Duration::from_millis(120), true);
        for section in [" CPU", " MEM", " DISK", " NET", " PROC"] {
            assert!(out.contains(section), "快照缺少 {section} 區塊");
        }
        assert!(out.contains("sysview"));
    }

    #[test]
    fn json_declares_permission_boundary() {
        let v = json(30, Duration::from_millis(120), true);
        assert!(
            v["permissions"]["note"].is_string(),
            "JSON 必須說明權限邊界"
        );
        assert_eq!(
            v["permissions"]["euid"].as_u64(),
            Some(crate::collectors::util::effective_uid() as u64)
        );
        // --json 不該包含任何管理員專屬資料
        assert!(
            v.get("admin").is_none(),
            "--json 不可繞過權限提供管理員資料"
        );
        assert!(v.get("user_storage").is_none());
    }

    #[test]
    fn json_has_stable_schema_and_provenance() {
        let v = json(30, Duration::from_millis(120), true);
        assert_eq!(v["schema"].as_u64(), Some(1));
        // 每個 reading 都要帶單位與品質標記
        let u = &v["cpu"]["usage"];
        assert!(u["unit"].is_string());
        assert!(u["quality"].is_string() || u["quality"].is_object());
    }

    #[test]
    fn json_is_serializable() {
        let v = json(30, Duration::from_millis(120), true);
        let s = serde_json::to_string(&v).expect("必須能序列化");
        assert!(s.len() > 200);
        let _: serde_json::Value = serde_json::from_str(&s).expect("必須能反序列化");
    }
}
