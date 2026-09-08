//! 「這台機器上的它現在是什麼」—— 把 metric id 對應到 collector 的當前值。
//!
//! 全部從已經取好的 state 讀，**不重新取樣、不執行任何指令**。
//! 拿不到就標成 `Unavailable` 並說明原因，不填 0 冒充。

use crate::collectors::SystemState;
use crate::metrics::describe::{Certainty, Field};
use crate::metrics::model::{Quality, Reading};
use crate::ui::format;

/// 把一個 [`Reading`] 轉成 Describe 的一列，順便把品質標記帶過去。
pub fn field(label: &str, r: &Reading) -> Field {
    let certainty = match &r.quality {
        Quality::Exact => Certainty::Exact,
        Quality::Estimated { .. } => Certainty::Estimated,
        Quality::Unavailable(_) => Certainty::Unavailable,
    };
    Field {
        label: label.to_owned(),
        value: format::reading(r),
        certainty,
    }
}

/// 某個 metric 在這台機器上的當前值。
pub fn for_metric(id: &str, s: &SystemState) -> Vec<Field> {
    let cpu = s.cpu.state();
    let mem = s.memory.state();
    let disk = s.disk.state();
    let net = s.network.state();
    let proc = s.process.state();
    let gpu = s.gpu.state();

    match id {
        // ── CPU ────────────────────────────────────────────────────────
        "cpu.usage" => vec![
            field("目前使用率", &cpu.usage),
            Field::exact("邏輯處理器", cpu.logical.to_string()),
            Field::exact("型號", cpu.model.clone()),
        ],
        "cpu.load" => vec![
            Field::exact(
                "1 / 5 / 15 分鐘",
                format!("{:.2}  {:.2}  {:.2}", cpu.load[0], cpu.load[1], cpu.load[2]),
            ),
            Field::derived(
                "每核負載",
                format!(
                    "{:.2}（load ÷ {} 核）",
                    cpu.load_per_core(),
                    cpu.logical.max(1)
                ),
            ),
        ],
        "cpu.model" => vec![
            Field::exact("型號", cpu.model.clone()),
            Field::exact("邏輯處理器", cpu.logical.to_string()),
            match cpu.physical {
                Some(n) => Field::exact("實體核心", n.to_string()),
                None => Field::unavailable("實體核心", "cpuinfo 沒有提供 core id"),
            },
        ],
        "cpu.logical" => {
            let mut v = vec![Field::exact("邏輯處理器", cpu.logical.to_string())];
            match cpu.physical {
                Some(n) => {
                    v.push(Field::exact("實體核心", n.to_string()));
                    v.push(Field::derived(
                        "每核執行緒",
                        format!("{:.0}", cpu.logical as f64 / n.max(1) as f64),
                    ));
                }
                None => v.push(Field::unavailable("實體核心", "cpuinfo 沒有提供 core id")),
            }
            v.push(Field::exact("型號", cpu.model.clone()));
            v
        }
        "cpu.load_per_core" => vec![
            Field::derived(
                "每核負載",
                format!(
                    "{:.2}（load {:.2} ÷ {} 核）",
                    cpu.load_per_core(),
                    cpu.load[0],
                    cpu.logical.max(1)
                ),
            ),
            Field::exact("1 分鐘 load", format!("{:.2}", cpu.load[0])),
            Field::exact("邏輯處理器", cpu.logical.to_string()),
        ],
        "cpu.intr" => vec![
            field("目前速率", &cpu.intr_rate),
            field("同期 context switch", &cpu.ctxt_rate),
        ],
        "sys.uptime" => vec![
            Field::exact("開機至今", crate::ui::format::duration(cpu.uptime_secs)),
            Field::exact("秒數", format!("{:.0}", cpu.uptime_secs)),
        ],
        "cpu.iowait" => vec![
            field("目前 iowait", &cpu.iowait),
            field("阻塞中的行程", &cpu.blocked),
        ],
        "cpu.steal" => vec![field("目前 steal", &cpu.steal)],
        "cpu.freq" => {
            let mut v = vec![field("平均頻率", &cpu.freq_avg())];
            let freqs: Vec<f64> = cpu.cores.iter().filter_map(|c| c.freq_mhz.get()).collect();
            if !freqs.is_empty() {
                let lo = freqs.iter().cloned().fold(f64::MAX, f64::min);
                let hi = freqs.iter().cloned().fold(0.0_f64, f64::max);
                v.push(Field::exact(
                    "目前範圍",
                    format!("{} – {}", format::hertz(lo), format::hertz(hi)),
                ));
            }
            v
        }
        "cpu.temp" => {
            let mut v = vec![field("封裝溫度", &cpu.temp)];
            if !cpu.core_temps.is_empty() {
                let hi = cpu
                    .core_temps
                    .iter()
                    .map(|(_, t)| *t)
                    .fold(0.0_f64, f64::max);
                v.push(Field::exact("最高核心溫度", format!("{hi:.0}°C")));
                v.push(Field::exact(
                    "有感測器的實體核心",
                    cpu.core_temps.len().to_string(),
                ));
                v.push(Field::exact(
                    "對得到溫度的邏輯處理器",
                    format!("{} / {}", cpu.cores_with_temp(), cpu.logical),
                ));
            }
            v
        }
        "cpu.ctxt" => vec![field("目前速率", &cpu.ctxt_rate)],
        "cpu.forks" => vec![field("目前速率", &cpu.fork_rate)],
        "cpu.running_blocked" => vec![
            field("可執行", &cpu.running),
            field("不可中斷 I/O 等待", &cpu.blocked),
        ],

        // ── Memory ─────────────────────────────────────────────────────
        "mem.used" => vec![
            field("使用率", &mem.used_percent()),
            Field::derived(
                "已用 / 總量",
                format!(
                    "{} / {}",
                    format::bytes(mem.used() as f64),
                    format::bytes(mem.total() as f64)
                ),
            ),
        ],
        "mem.available" => vec![
            Field::exact("MemAvailable", format::bytes(mem.available() as f64)),
            Field::exact("MemFree", format::bytes(mem.free() as f64)),
        ],
        "mem.cached" => vec![
            Field::derived("可回收快取", format::bytes(mem.reclaimable_cache() as f64)),
            Field::exact("Buffers", format::bytes(mem.buffers() as f64)),
            Field::exact(
                "Shmem（不可回收，已扣除）",
                format::bytes(mem.get("Shmem").unwrap_or(0) as f64),
            ),
        ],
        "mem.swap" => {
            if mem.swap_total() == 0 {
                vec![Field::exact("Swap", "這台機器沒有設定 swap")]
            } else {
                vec![
                    field("使用率", &mem.swap_percent()),
                    Field::derived(
                        "已用 / 總量",
                        format!(
                            "{} / {}",
                            format::bytes(mem.swap_used() as f64),
                            format::bytes(mem.swap_total() as f64)
                        ),
                    ),
                ]
            }
        }

        // ── cgroup ─────────────────────────────────────────────────────
        "cgroup.cpu" => match s.cgroup.cpu_limit_cores {
            Some(c) => vec![
                Field::exact("CPU 配額", format!("{c:.2} 核")),
                Field::exact("執行環境", s.cgroup.environment.label().to_owned()),
            ],
            None => vec![Field::exact("CPU 配額", "沒有限制（cpu.max = max）")],
        },
        "cgroup.memory" => match (s.cgroup.memory_limit, s.cgroup.memory_current) {
            (Some(l), Some(c)) => vec![
                Field::exact("記憶體上限", format::bytes(l as f64)),
                Field::exact("目前用量", format::bytes(c as f64)),
            ],
            _ => vec![Field::exact("記憶體上限", "沒有限制（memory.max = max）")],
        },

        // ── Storage ────────────────────────────────────────────────────
        "disk.usage" | "disk.inodes" => disk
            .mounts
            .iter()
            .map(|m| {
                let r = if id == "disk.usage" {
                    &m.usage
                } else {
                    &m.inode_usage
                };
                field(&m.mount_point, r)
            })
            .collect(),
        "disk.throughput" => vec![
            field("全部裝置讀取", &disk.total_read),
            field("全部裝置寫入", &disk.total_write),
        ],
        "disk.iops" | "disk.util" => disk
            .devices
            .iter()
            .map(|d| {
                let r = if id == "disk.iops" { &d.iops } else { &d.util };
                field(&d.name, r)
            })
            .collect(),

        // ── Network ────────────────────────────────────────────────────
        "net.throughput" => vec![
            field("實體介面總下載", &net.total_rx),
            field("實體介面總上傳", &net.total_tx),
        ],
        "net.mac" => net
            .interfaces
            .iter()
            .map(|i| match &i.mac {
                Some(m) => Field::exact(i.name.clone(), m.clone()),
                None => Field::unavailable(i.name.clone(), "這個介面沒有硬體位址"),
            })
            .collect(),
        "net.ip" => {
            let mut v = Vec::new();
            for i in &net.interfaces {
                if i.addrs.is_empty() {
                    v.push(Field::unavailable(i.name.clone(), "沒有設定位址"));
                } else {
                    v.push(Field::exact(i.name.clone(), i.addrs.join("  ")));
                }
            }
            v
        }
        "net.errors" => net
            .interfaces
            .iter()
            .filter(|i| !i.virtual_iface)
            .map(|i| {
                Field::exact(
                    i.name.clone(),
                    format!("錯誤 {} / 丟包 {}", i.errors, i.drops),
                )
            })
            .collect(),
        "net.sockets" => {
            let s2 = &net.sockets;
            let g = |v: Option<u64>| match v {
                Some(x) => x.to_string(),
                None => "n/a".to_owned(),
            };
            vec![
                Field::exact("TCP in use", g(s2.tcp_inuse)),
                Field::exact("TCP TIME_WAIT", g(s2.tcp_tw)),
                Field::exact("UDP in use", g(s2.udp_inuse)),
                Field::exact("全部 socket", g(s2.sockets_used)),
            ]
        }

        // ── GPU ────────────────────────────────────────────────────────
        "gpu.util" | "gpu.vram" | "gpu.temp" | "gpu.power" | "gpu.clock" | "gpu.pstate"
        | "gpu.pcie" | "gpu.intel_util" | "gpu.fan" => gpu
            .devices
            .iter()
            .map(|d| {
                let r = match id {
                    "gpu.vram" => &d.memory_used,
                    "gpu.temp" => &d.temperature,
                    "gpu.power" => &d.power,
                    "gpu.clock" => &d.sm_clock,
                    "gpu.fan" => &d.fan,
                    _ => &d.utilization,
                };
                let mut f = field(&d.name, r);
                if id == "gpu.pstate" {
                    f.value = d.pstate.clone().unwrap_or_else(|| "n/a".into());
                    f.certainty = if d.pstate.is_some() {
                        Certainty::Exact
                    } else {
                        Certainty::Unavailable
                    };
                } else if id == "gpu.pcie" {
                    f.value = match (d.pcie_gen, d.pcie_width) {
                        (Some(g), Some(w)) => format!("gen{g} x{w}"),
                        _ => "n/a".into(),
                    };
                    f.certainty = if d.pcie_gen.is_some() {
                        Certainty::Exact
                    } else {
                        Certainty::Unavailable
                    };
                }
                f
            })
            .collect(),
        "gpu.process" => {
            let mut v = Vec::new();
            for d in &gpu.devices {
                if d.processes.is_empty() {
                    continue;
                }
                for p in &d.processes {
                    v.push(Field::exact(
                        format!("{} · PID {}", d.name, p.pid),
                        format!(
                            "{} · {}",
                            p.name.clone().unwrap_or_else(|| "（看不到名稱）".into()),
                            p.used_memory
                                .map(|m| format::bytes(m as f64))
                                .unwrap_or_else(|| "n/a".into())
                        ),
                    ));
                }
                if !d.process_list_complete {
                    v.push(Field::unavailable(
                        "完整清單",
                        "只列得出你自己的行程，其他使用者的需要管理員權限",
                    ));
                }
            }
            if v.is_empty() {
                v.push(Field::exact("目前", "沒有任何 compute 行程在使用 GPU"));
            }
            v
        }

        // ── Process ────────────────────────────────────────────────────
        "proc.count" => vec![
            Field::exact("行程數", proc.total.to_string()),
            Field::exact("執行緒數", proc.threads.to_string()),
        ],
        "proc.state" => {
            let mut v: Vec<_> = proc.by_state.iter().collect();
            v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
            v.into_iter()
                .take(6)
                .map(|(c, n)| Field::exact(format!("{c}（{}）", state_word(*c)), n.to_string()))
                .collect()
        }
        "proc.cpu" | "proc.rss" | "proc.virt" | "proc.threads" => {
            use crate::collectors::process::SortKey;
            let key = if id == "proc.rss" {
                SortKey::Memory
            } else {
                SortKey::Cpu
            };
            proc.top(key, 5, "")
                .into_iter()
                .map(|p| {
                    let val = match id {
                        "proc.rss" => format::bytes(p.rss as f64),
                        "proc.virt" => format::bytes(p.vsize as f64),
                        "proc.threads" => p.threads.to_string(),
                        _ => format!("{:.1}%", p.cpu_percent),
                    };
                    Field::exact(format!("{} ({})", p.name, p.pid), val)
                })
                .collect()
        }

        // ── 管理員擴充 ─────────────────────────────────────────────────
        "storage.user" => vec![Field::unavailable(
            "各使用者用量",
            "需要管理員權限。到 Admin 頁按 u 解鎖後才會掃描。",
        )],

        _ => Vec::new(),
    }
}

fn state_word(c: char) -> &'static str {
    match c {
        'R' => "執行中",
        'S' => "可中斷睡眠",
        'D' => "不可中斷 I/O",
        'Z' => "殭屍",
        'T' => "停止",
        't' => "追蹤中",
        'I' => "閒置",
        _ => "其他",
    }
}
