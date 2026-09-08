//! `sysview-priv` — sysview 的最小化特權 helper。
//!
//! # 這個程式的全部行為
//!
//! 由 `sudo` 以 root 執行，做完 allowlist 上的**一件**事，把 JSON 印到 stdout，
//! 然後結束。沒有常駐、沒有 socket、沒有 shell、沒有 `exec`。
//!
//! # 威脅模型
//!
//! **假設呼叫端（sysview TUI）已經被攻陷。** 它是一般使用者身分執行的程式，
//! 攻擊者可以任意控制傳進來的參數。因此這裡：
//!
//! * 重新驗證每一個參數，完全不信任呼叫端驗過了；
//! * 不接受任何路徑 —— 家目錄一律自己查 password database；
//! * 不接受任何指令字串，不 spawn shell；
//! * 訊號只認三個名字的 allowlist，不接受數字；
//! * 會改狀態的操作先用 `(pid, starttime)` 重新確認目標，避免 PID 重用；
//! * 絕不讀 `/proc/<pid>/environ`（那裡面常有 token 與密碼）；
//! * 絕不寫任何暫存檔或快取到磁碟。
//!
//! # 絕不 setuid
//!
//! 這個檔案安裝成 `root:root 0755`，**沒有 setuid 位元**。
//! 唯一的提權途徑是 `sudo`，這樣授權決定就會落在 sudoers / PAM / LDAP 手上，
//! 而不是我們自己發明一套。沒有 root EUID 時直接拒絕執行。

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use serde_json::json;
use sysview::collectors::storage_scan::{self, Cancel, ScanLimits};
use sysview::collectors::util;
use sysview::privilege::protocol::{
    validate_pid, validate_username, Operation, Response, SafeSignal, ValidationError,
    PROTOCOL_VERSION,
};

fn main() -> std::process::ExitCode {
    // helper 的輸出是 JSON，一樣可能被提早關閉的管線接住。
    util::restore_default_sigpipe();

    let argv: Vec<String> = std::env::args().skip(1).collect();

    // ── 第一道關卡：沒有 root EUID 就什麼都不做 ────────────────────────
    // 一般使用者直接執行這個 binary 會停在這裡。
    if util::effective_uid() != 0 {
        emit(Response::error(
            "startup",
            "sysview-priv 必須以 root 執行。請不要直接呼叫它 —— \
             sysview 會在需要時透過 sudo 代為執行。",
        ));
        return std::process::ExitCode::from(77); // EX_NOPERM
    }

    let op = match parse_argv(&argv) {
        Ok(op) => op,
        Err(e) => {
            emit(Response::error("parse", e.to_string()));
            return std::process::ExitCode::from(64); // EX_USAGE
        }
    };

    // ── 第二道關卡：不信任呼叫端，自己再驗一次 ──────────────────────────
    if let Err(e) = op.validate() {
        audit(&op, &format!("rejected: {e}"));
        emit(Response::error(op.name(), e.to_string()));
        return std::process::ExitCode::from(64);
    }

    let name = op.name();
    let result = dispatch(&op);

    match result {
        Ok(data) => {
            if op.is_mutation() {
                audit(&op, "ok");
            }
            emit(Response::ok(name, data));
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            if op.is_mutation() {
                audit(&op, &format!("failed: {e}"));
            }
            emit(Response::error(name, e.to_string()));
            std::process::ExitCode::FAILURE
        }
    }
}

fn emit(r: Response) {
    match serde_json::to_string(&r) {
        Ok(s) => println!("{s}"),
        // 連序列化都失敗的話，至少給一個合法的 JSON
        Err(_) => println!(
            r#"{{"protocol_version":{PROTOCOL_VERSION},"operation":"unknown","status":"error","message":"serialization failed"}}"#
        ),
    }
}

/// 把 argv 解析成 [`Operation`]。
///
/// **這是唯一的入口**。刻意手寫而不用 clap：介面極小，手寫比較容易一眼看出
/// 「這裡不可能接受到別的東西」。任何無法辨識的東西一律拒絕，不做猜測。
fn parse_argv(argv: &[String]) -> Result<Operation, ValidationError> {
    let Some(sub) = argv.first() else {
        return Err(ValidationError::UnknownOperation);
    };
    let flags = parse_flags(&argv[1..])?;
    // 每個操作只認自己的參數；多給的一律拒絕 ——「多一個沒關係」是 parser 變寬的起點
    let allowed: &[&str] = match sub.as_str() {
        "storage-user-detail" => &["user"],
        "process-detail" => &["pid"],
        "process-signal" => &["pid", "starttime", "signal"],
        "renice" => &["pid", "starttime", "nice"],
        _ => &[],
    };
    if flags.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(ValidationError::UnknownOperation);
    }

    let need = |k: &str| -> Result<&String, ValidationError> {
        flags.get(k).ok_or(ValidationError::UnknownOperation)
    };
    let need_pid = || -> Result<i32, ValidationError> {
        let v: i32 = need("pid")?.parse().map_err(|_| ValidationError::BadPid)?;
        validate_pid(v)?;
        Ok(v)
    };
    let need_starttime = || -> Result<u64, ValidationError> {
        need("starttime")?
            .parse()
            .map_err(|_| ValidationError::BadStarttime)
    };

    match sub.as_str() {
        "capability" => Ok(Operation::Capability),
        "storage-users" => Ok(Operation::StorageUsers),
        "storage-user-detail" => {
            let user = need("user")?.clone();
            validate_username(&user)?;
            Ok(Operation::StorageUserDetail { user })
        }
        "user-memory" => Ok(Operation::UserMemory),
        "gpu-users" => Ok(Operation::GpuUsers),
        "socket-map" => Ok(Operation::SocketMap),
        "process-detail" => Ok(Operation::ProcessDetail { pid: need_pid()? }),
        "process-signal" => Ok(Operation::ProcessSignal {
            pid: need_pid()?,
            starttime: need_starttime()?,
            signal: SafeSignal::parse(need("signal")?)?,
        }),
        "renice" => Ok(Operation::Renice {
            pid: need_pid()?,
            starttime: need_starttime()?,
            nice: need("nice")?
                .parse()
                .map_err(|_| ValidationError::NiceOutOfRange)?,
        }),
        // 這裡沒有 catch-all 的 "exec"、"run"、"shell"。故意的。
        _ => Err(ValidationError::UnknownOperation),
    }
}

/// 解析 `--key value` 形式的參數。
///
/// 只接受 `--key value`：不支援 `--key=value`、不支援短選項、不支援位置參數。
/// 介面越窄，能出錯的地方越少。
fn parse_flags(args: &[String]) -> Result<HashMap<String, String>, ValidationError> {
    let mut out = HashMap::new();
    let mut i = 0;
    while i < args.len() {
        let Some(key) = args[i].strip_prefix("--") else {
            return Err(ValidationError::UnknownOperation);
        };
        if key.is_empty() || key.contains('=') {
            return Err(ValidationError::UnknownOperation);
        }
        let Some(val) = args.get(i + 1) else {
            return Err(ValidationError::UnknownOperation);
        };
        if out.insert(key.to_owned(), val.clone()).is_some() {
            // 同一個參數給兩次是可疑的，直接拒絕
            return Err(ValidationError::UnknownOperation);
        }
        i += 2;
    }
    Ok(out)
}

fn dispatch(op: &Operation) -> anyhow::Result<serde_json::Value> {
    match op {
        Operation::Capability => Ok(json!({
            "protocol_version": PROTOCOL_VERSION,
            "helper_version": sysview::VERSION,
            "euid": util::effective_uid(),
            "operations": [
                "capability", "storage-users", "storage-user-detail",
                "user-memory", "gpu-users", "socket-map", "process-detail",
                "process-signal", "renice"
            ],
        })),
        Operation::StorageUsers => storage_users(),
        Operation::StorageUserDetail { user } => storage_user_detail(user),
        Operation::UserMemory => user_memory(),
        Operation::GpuUsers => gpu_users(),
        Operation::SocketMap => socket_map(),
        Operation::ProcessDetail { pid } => process_detail(*pid),
        Operation::ProcessSignal {
            pid,
            starttime,
            signal,
        } => process_signal(*pid, *starttime, *signal),
        Operation::Renice {
            pid,
            starttime,
            nice,
        } => renice(*pid, *starttime, *nice),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// password database
// ─────────────────────────────────────────────────────────────────────────────

struct PasswdEntry {
    name: String,
    uid: u32,
    home: PathBuf,
}

/// 列出「真人」帳號。
///
/// uid < 1000 是系統帳號（daemon、www-data…），nobody 是 65534。
/// 這些不是我們關心的對象，也避免把 /var/lib 之類的目錄算成使用者資料。
fn real_users() -> Vec<PasswdEntry> {
    let mut out = Vec::new();
    // SAFETY: setpwent/getpwent/endpwent 是標準的 passwd 走訪 API。
    // 回傳的指標指向靜態緩衝區，我們在下一次呼叫前就把資料複製走。
    unsafe {
        libc::setpwent();
        loop {
            let pw = libc::getpwent();
            if pw.is_null() {
                break;
            }
            let uid = (*pw).pw_uid;
            if !(1000..65534).contains(&uid) {
                continue;
            }
            let name = cstr((*pw).pw_name);
            let home = cstr((*pw).pw_dir);
            if name.is_empty() || home.is_empty() || home == "/" {
                continue;
            }
            out.push(PasswdEntry {
                name,
                uid,
                home: PathBuf::from(home),
            });
        }
        libc::endpwent();
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.uid == b.uid);
    out
}

/// 依名稱查單一使用者。**這是唯一把名稱變成路徑的地方** ——
/// 路徑來自 password database 而不是呼叫端，所以路徑穿越不可能發生。
fn lookup_user(name: &str) -> Option<PasswdEntry> {
    validate_username(name).ok()?;
    let c = std::ffi::CString::new(name).ok()?;
    // SAFETY: c 是合法 NUL 結尾字串；回傳指標在下次呼叫前有效，我們立即複製。
    unsafe {
        let pw = libc::getpwnam(c.as_ptr());
        if pw.is_null() {
            return None;
        }
        let uid = (*pw).pw_uid;
        let home = cstr((*pw).pw_dir);
        if home.is_empty() || home == "/" {
            return None;
        }
        Some(PasswdEntry {
            name: cstr((*pw).pw_name),
            uid,
            home: PathBuf::from(home),
        })
    }
}

/// SAFETY: 呼叫端保證 `p` 是 NUL 結尾的 C 字串或 null。
unsafe fn cstr(p: *const libc::c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// 各項操作
// ─────────────────────────────────────────────────────────────────────────────

/// 每位使用者的家目錄用量。有整體預算上限，避免掃到天荒地老。
fn storage_users() -> anyhow::Result<serde_json::Value> {
    let users = real_users();
    let cancel = Cancel::new();
    // 整體 90 秒預算，平均分給每個使用者，至少每人 5 秒。
    let per_user = std::time::Duration::from_secs((90 / users.len().max(1) as u64).max(5));
    let limits = ScanLimits {
        deadline: per_user,
        ..Default::default()
    };

    let mut results = Vec::new();
    for u in &users {
        let r = storage_scan::scan_directory(&u.home, limits, &cancel);
        results.push(json!({
            "user": u.name,
            "uid": u.uid,
            "home": u.home.to_string_lossy(),
            "bytes": r.bytes,
            "entries": r.entries,
            "denied": r.denied,
            "complete": r.stop.is_complete(),
            "stop_reason": r.stop.label(),
            "elapsed_ms": r.elapsed_ms,
        }));
    }
    Ok(json!({ "users": results }))
}

/// 單一使用者家目錄的第一層明細（drill-down）。
fn storage_user_detail(user: &str) -> anyhow::Result<serde_json::Value> {
    let Some(u) = lookup_user(user) else {
        anyhow::bail!("找不到使用者 {user}");
    };
    let limits = ScanLimits {
        deadline: std::time::Duration::from_secs(45),
        ..Default::default()
    };
    let r = storage_scan::scan_directory(&u.home, limits, &Cancel::new());
    Ok(json!({
        "user": u.name,
        "uid": u.uid,
        "home": u.home.to_string_lossy(),
        "bytes": r.bytes,
        "complete": r.stop.is_complete(),
        "stop_reason": r.stop.label(),
        "elapsed_ms": r.elapsed_ms,
        "children": r.children.iter().map(|c| json!({
            "name": c.name, "bytes": c.bytes, "entries": c.entries,
        })).collect::<Vec<_>>(),
    }))
}

/// 每位使用者的記憶體用量彙總。
///
/// root 讀得到 `smaps_rollup`，所以能給出 **PSS**（共享頁面按比例分攤），
/// 那比 RSS 公平得多 —— RSS 把共享函式庫在每個行程都算一次。
fn user_memory() -> anyhow::Result<serde_json::Value> {
    #[derive(Default)]
    struct Agg {
        rss: u64,
        pss: u64,
        pss_available: bool,
        procs: u64,
    }
    let mut per_uid: HashMap<u32, Agg> = HashMap::new();
    let page = util::page_size();

    for pid in iter_pids() {
        let base = format!("/proc/{pid}");
        let Ok(md) = std::fs::metadata(&base) else {
            continue;
        };
        let uid = md.uid();
        let Ok(stat) = util::read_string(format!("{base}/stat")) else {
            continue;
        };
        let Some(st) = sysview::collectors::process::parse_proc_stat(&stat) else {
            continue;
        };
        let e = per_uid.entry(uid).or_default();
        e.rss += (st.rss_pages.max(0) as u64).saturating_mul(page);
        e.procs += 1;
        // smaps_rollup 比 RSS 準但貴得多；讀不到就算了（核心執行緒沒有）
        if let Ok(roll) = util::read_string(format!("{base}/smaps_rollup")) {
            if let Some(kb) = roll.lines().find_map(|l| {
                l.strip_prefix("Pss:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            }) {
                e.pss += kb * 1024;
                e.pss_available = true;
            }
        }
    }

    let mut users: Vec<_> = per_uid
        .into_iter()
        .map(|(uid, a)| {
            json!({
                "uid": uid,
                "user": util::username(uid),
                "rss": a.rss,
                "pss": if a.pss_available { Some(a.pss) } else { None },
                "processes": a.procs,
            })
        })
        .collect();
    users.sort_by_key(|v| std::cmp::Reverse(v["rss"].as_u64().unwrap_or(0)));
    Ok(json!({ "users": users }))
}

/// 每位使用者的 GPU VRAM 用量。
fn gpu_users() -> anyhow::Result<serde_json::Value> {
    use nvml_wrapper::enums::device::UsedGpuMemory;
    let nvml = match nvml_wrapper::Nvml::init() {
        Ok(n) => n,
        Err(e) => return Ok(json!({ "available": false, "reason": e.to_string() })),
    };
    let mut per_uid: HashMap<u32, (u64, u64)> = HashMap::new(); // uid -> (vram, procs)
    let mut devices = Vec::new();
    for i in 0..nvml.device_count().unwrap_or(0) {
        let Ok(dev) = nvml.device_by_index(i) else {
            continue;
        };
        let name = dev.name().unwrap_or_else(|_| format!("GPU {i}"));
        let procs = dev.running_compute_processes().unwrap_or_default();
        let mut listed = Vec::new();
        for p in procs {
            let mem = match p.used_gpu_memory {
                UsedGpuMemory::Used(b) => b,
                UsedGpuMemory::Unavailable => 0,
            };
            // root 讀得到所有行程的擁有者
            let uid = std::fs::metadata(format!("/proc/{}", p.pid))
                .map(|m| m.uid())
                .ok();
            if let Some(uid) = uid {
                let e = per_uid.entry(uid).or_default();
                e.0 += mem;
                e.1 += 1;
            }
            listed.push(json!({
                "pid": p.pid,
                "used_memory": mem,
                "uid": uid,
                "user": uid.map(util::username),
                "name": comm_of(p.pid as i32),
            }));
        }
        devices.push(json!({ "index": i, "name": name, "processes": listed }));
    }
    let mut users: Vec<_> = per_uid
        .into_iter()
        .map(|(uid, (vram, n))| {
            json!({ "uid": uid, "user": util::username(uid), "vram": vram, "processes": n })
        })
        .collect();
    users.sort_by_key(|v| std::cmp::Reverse(v["vram"].as_u64().unwrap_or(0)));
    Ok(json!({ "available": true, "devices": devices, "users": users }))
}

/// socket → PID → 使用者 對應。
///
/// 只回報**中繼資料**（誰開了幾條連線、哪些 port 在監聽）。
/// **不做封包擷取、不看內容** —— 這不是 Wireshark。
fn socket_map() -> anyhow::Result<serde_json::Value> {
    // 1. 從 /proc/net/* 收集 socket inode → 狀態
    let mut inode_state: HashMap<u64, (&'static str, &'static str)> = HashMap::new();
    for (file, proto) in [
        ("/proc/net/tcp", "tcp"),
        ("/proc/net/tcp6", "tcp6"),
        ("/proc/net/udp", "udp"),
        ("/proc/net/udp6", "udp6"),
    ] {
        let Ok(text) = util::read_string(file) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_ascii_whitespace().collect();
            if f.len() < 10 {
                continue;
            }
            let state = if proto.starts_with("tcp") {
                match f[3] {
                    "01" => "ESTABLISHED",
                    "0A" => "LISTEN",
                    "06" => "TIME_WAIT",
                    "08" => "CLOSE_WAIT",
                    _ => "OTHER",
                }
            } else {
                "UDP"
            };
            if let Ok(ino) = f[9].parse::<u64>() {
                inode_state.insert(ino, (proto, state));
            }
        }
    }

    // 2. 掃 /proc/*/fd 找出誰持有這些 inode
    let mut per_uid: HashMap<u32, HashMap<&'static str, u64>> = HashMap::new();
    for pid in iter_pids() {
        let Ok(md) = std::fs::metadata(format!("/proc/{pid}")) else {
            continue;
        };
        let uid = md.uid();
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            // read_link 不會開啟目標，所以不會阻塞在 fifo/device 上
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let t = target.to_string_lossy();
            let Some(ino) = t.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')) else {
                continue;
            };
            let Ok(ino) = ino.parse::<u64>() else {
                continue;
            };
            if let Some((_, state)) = inode_state.get(&ino) {
                *per_uid.entry(uid).or_default().entry(state).or_insert(0) += 1;
            }
        }
    }

    let mut users: Vec<_> = per_uid
        .into_iter()
        .map(|(uid, states)| {
            json!({
                "uid": uid,
                "user": util::username(uid),
                "established": states.get("ESTABLISHED").copied().unwrap_or(0),
                "listen": states.get("LISTEN").copied().unwrap_or(0),
                "time_wait": states.get("TIME_WAIT").copied().unwrap_or(0),
                "udp": states.get("UDP").copied().unwrap_or(0),
                "total": states.values().sum::<u64>(),
            })
        })
        .collect();
    users.sort_by_key(|v| std::cmp::Reverse(v["total"].as_u64().unwrap_or(0)));
    Ok(json!({ "users": users, "sockets_tracked": inode_state.len() }))
}

/// 單一行程的深入資訊。
///
/// **刻意不包含 `/proc/<pid>/environ`** —— 環境變數裡幾乎一定有
/// API token、資料庫密碼之類的東西，管理員也沒有理由在監控畫面上看到它們。
fn process_detail(pid: i32) -> anyhow::Result<serde_json::Value> {
    let base = format!("/proc/{pid}");
    let md =
        std::fs::metadata(&base).map_err(|_| anyhow::anyhow!("行程 {pid} 不存在（可能剛結束）"))?;
    let uid = md.uid();
    let stat = util::read_string(format!("{base}/stat"))
        .map_err(|e| anyhow::anyhow!("讀不到 stat: {e}"))?;
    let st = sysview::collectors::process::parse_proc_stat(&stat)
        .ok_or_else(|| anyhow::anyhow!("stat 格式無法解析"))?;

    let fd_count = std::fs::read_dir(format!("{base}/fd"))
        .map(|d| d.flatten().count() as u64)
        .unwrap_or(0);

    let status = util::read_string(format!("{base}/status")).unwrap_or_default();
    let field = |k: &str| -> Option<String> {
        status.lines().find_map(|l| {
            l.strip_prefix(k)?
                .trim()
                .split('\t')
                .next()
                .map(str::to_owned)
        })
    };

    Ok(json!({
        "pid": pid,
        "ppid": st.ppid,
        "uid": uid,
        "user": util::username(uid),
        "name": st.comm,
        "state": st.state.to_string(),
        "threads": st.num_threads,
        "starttime": st.starttime,
        "nice": st.nice,
        "cgroup": util::read_trimmed(format!("{base}/cgroup")).ok(),
        "cpu_affinity": field("Cpus_allowed_list:"),
        "scheduler": util::read_trimmed(format!("{base}/sched"))
            .ok()
            .and_then(|s| s.lines().next().map(str::to_owned)),
        "open_fds": fd_count,
        "voluntary_ctxt_switches": field("voluntary_ctxt_switches:"),
        "nonvoluntary_ctxt_switches": field("nonvoluntary_ctxt_switches:"),
        "rss_bytes": (st.rss_pages.max(0) as u64) * util::page_size(),
        "vsize_bytes": st.vsize,
        // environ 刻意不提供 —— 見上方註解
        "environ": serde_json::Value::Null,
        "environ_note": "環境變數常含機密，sysview 刻意不讀取",
    }))
}

/// 確認 `(pid, starttime)` 仍指向同一個行程。
///
/// PID 會被重用。UI 一秒前看到的 PID 12345 可能已經結束，現在的 12345 是別的
/// 行程。`starttime`（開機以來的 jiffies）加上 PID 幾乎可以唯一識別一個行程，
/// 所以動手之前一定要重新比對 —— 這是防止 confused deputy 的關鍵。
fn confirm_target(pid: i32, starttime: u64) -> anyhow::Result<(u32, String)> {
    let base = format!("/proc/{pid}");
    let stat = util::read_string(format!("{base}/stat"))
        .map_err(|_| anyhow::anyhow!("行程 {pid} 已經不存在"))?;
    let st = sysview::collectors::process::parse_proc_stat(&stat)
        .ok_or_else(|| anyhow::anyhow!("行程 {pid} 的 stat 無法解析"))?;
    if st.starttime != starttime {
        anyhow::bail!(
            "PID {pid} 已經被重用（starttime 期望 {starttime}，實際 {}）—— 拒絕操作",
            st.starttime
        );
    }
    let uid = std::fs::metadata(&base)
        .map(|m| m.uid())
        .map_err(|_| anyhow::anyhow!("行程 {pid} 已經不存在"))?;
    Ok((uid, st.comm))
}

fn process_signal(
    pid: i32,
    starttime: u64,
    signal: SafeSignal,
) -> anyhow::Result<serde_json::Value> {
    let (uid, comm) = confirm_target(pid, starttime)?;
    // 不對 init 動手。殺掉 PID 1 會讓整台機器停擺。
    if pid == 1 {
        anyhow::bail!("拒絕對 PID 1 (init) 送出訊號");
    }
    // SAFETY: pid 已驗證為正數，signal 來自 allowlist enum。
    let rc = unsafe { libc::kill(pid, signal.number()) };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        anyhow::bail!("kill({pid}, {}) 失敗：{e}", signal.name());
    }
    Ok(json!({
        "pid": pid, "signal": signal.name(),
        "target_uid": uid, "target_user": util::username(uid), "target_comm": comm,
    }))
}

fn renice(pid: i32, starttime: u64, nice: i8) -> anyhow::Result<serde_json::Value> {
    let (uid, comm) = confirm_target(pid, starttime)?;
    // SAFETY: pid 已驗證，nice 已限制在 0..=19。
    let rc =
        unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, nice as libc::c_int) };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        anyhow::bail!("setpriority({pid}, {nice}) 失敗：{e}");
    }
    Ok(json!({
        "pid": pid, "nice": nice,
        "target_uid": uid, "target_user": util::username(uid), "target_comm": comm,
    }))
}

fn iter_pids() -> Vec<i32> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .collect()
}

fn comm_of(pid: i32) -> Option<String> {
    util::read_trimmed(format!("/proc/{pid}/comm")).ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Audit
// ─────────────────────────────────────────────────────────────────────────────

/// 把會改變系統狀態的操作寫進 syslog / journald。
///
/// **只記中繼資料**：時間、誰呼叫的、對誰做了什麼、結果。
/// 不記密碼、不記環境變數、不記完整命令列 —— 那些可能含機密。
///
/// `sudo` 本身也會留下自己的 log，兩者互相佐證。
fn audit(op: &Operation, result: &str) {
    // 在 sudo 底下 getuid() 已經是 0：真正的呼叫者只在 sudo 留下的環境變數裡。
    let who = format!("uid={}{}", util::real_uid(), sudo_caller());
    let msg = match op {
        Operation::ProcessSignal { pid, signal, .. } => format!(
            "{who} op={} target_pid={pid} signal={} result={result}",
            op.name(),
            signal.name()
        ),
        Operation::Renice { pid, nice, .. } => format!(
            "{who} op={} target_pid={pid} nice={nice} result={result}",
            op.name()
        ),
        _ => format!("{who} op={} result={result}", op.name()),
    };
    write_syslog(&msg);
}

/// sudo 留下的呼叫者資訊（`SUDO_UID` / `SUDO_USER`），**只拿來記錄，不拿來授權**：
/// 授權是 sudoers 的事，這兩個變數任何人都能自己設。sudo 自己的 log 才是權威，
/// 這裡只是讓 `journalctl -t sysview-priv` 一眼看得出是誰。
fn sudo_caller() -> String {
    let uid = std::env::var("SUDO_UID")
        .ok()
        .filter(|s| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()));
    let user = std::env::var("SUDO_USER")
        .ok()
        .filter(|s| sysview::privilege::protocol::validate_username(s).is_ok());
    let mut out = String::new();
    if let Some(u) = uid {
        out.push_str(&format!(" sudo_uid={u}"));
    }
    if let Some(n) = user {
        out.push_str(&format!(" sudo_user={n}"));
    }
    out
}

fn write_syslog(msg: &str) {
    let Ok(ident) = std::ffi::CString::new("sysview-priv") else {
        return;
    };
    let Ok(m) = std::ffi::CString::new(msg) else {
        return;
    };
    let Ok(fmt) = std::ffi::CString::new("%s") else {
        return;
    };
    // SAFETY: 三個字串都是合法的 NUL 結尾 C 字串；用 "%s" 當格式字串，
    // 所以訊息裡的 % 不會被當成格式指示符（format string 攻擊防護）。
    unsafe {
        libc::openlog(ident.as_ptr(), libc::LOG_PID, libc::LOG_AUTHPRIV);
        libc::syslog(libc::LOG_NOTICE, fmt.as_ptr(), m.as_ptr());
        libc::closelog();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_all_allowlisted_operations() {
        assert_eq!(
            parse_argv(&argv(&["capability"])).unwrap(),
            Operation::Capability
        );
        assert_eq!(
            parse_argv(&argv(&["storage-users"])).unwrap(),
            Operation::StorageUsers
        );
        assert_eq!(
            parse_argv(&argv(&["user-memory"])).unwrap(),
            Operation::UserMemory
        );
        assert_eq!(
            parse_argv(&argv(&["gpu-users"])).unwrap(),
            Operation::GpuUsers
        );
        assert_eq!(
            parse_argv(&argv(&["socket-map"])).unwrap(),
            Operation::SocketMap
        );
        assert_eq!(
            parse_argv(&argv(&["process-detail", "--pid", "42"])).unwrap(),
            Operation::ProcessDetail { pid: 42 }
        );
    }

    #[test]
    fn rejects_arbitrary_command_execution() {
        // 這些是攻擊者最想要的入口，一個都不能存在
        for bad in [
            vec!["exec", "--cmd", "id"],
            vec!["run", "--command", "/bin/sh"],
            vec!["shell"],
            vec!["sh", "-c", "id"],
            vec!["eval", "--expr", "1"],
            vec!["--", "/bin/sh"],
        ] {
            assert!(
                parse_argv(&argv(&bad)).is_err(),
                "{bad:?} 必須被拒絕 —— helper 不是任意指令的閘道"
            );
        }
    }

    #[test]
    fn rejects_extra_flags_even_when_well_formed() {
        assert!(parse_argv(&argv(&["capability", "--banana", "value"])).is_err());
        assert!(parse_argv(&argv(&["process-detail", "--pid", "42", "--unused", "x"])).is_err());
        assert!(parse_argv(&argv(&[
            "renice",
            "--pid",
            "42",
            "--starttime",
            "9",
            "--nice",
            "5",
            "--extra",
            "1"
        ]))
        .is_err());
        assert!(parse_argv(&argv(&["process-detail", "--pid", "42"])).is_ok());
    }

    #[test]
    fn rejects_unknown_subcommands() {
        assert!(parse_argv(&argv(&["nope"])).is_err());
        assert!(parse_argv(&argv(&[])).is_err());
        assert!(parse_argv(&argv(&["storage-users-x"])).is_err());
    }

    #[test]
    fn rejects_path_arguments_entirely() {
        // 協定裡沒有任何路徑欄位，所以連傳都傳不進來
        assert!(parse_argv(&argv(&["storage-user-detail", "--path", "/etc"])).is_err());
        assert!(parse_argv(&argv(&["storage-user-detail", "--user", "../../etc"])).is_err());
        assert!(parse_argv(&argv(&["storage-user-detail", "--user", "/root"])).is_err());
    }

    #[test]
    fn rejects_signal_numbers_and_kill() {
        for sig in ["9", "KILL", "SIGKILL", "-9", "0"] {
            let r = parse_argv(&argv(&[
                "process-signal",
                "--pid",
                "100",
                "--starttime",
                "5",
                "--signal",
                sig,
            ]));
            assert!(r.is_err(), "訊號 {sig:?} 不在 allowlist 上，必須拒絕");
        }
        assert!(parse_argv(&argv(&[
            "process-signal",
            "--pid",
            "100",
            "--starttime",
            "5",
            "--signal",
            "TERM"
        ]))
        .is_ok());
    }

    #[test]
    fn rejects_pid_zero_which_would_signal_whole_group() {
        assert!(parse_argv(&argv(&["process-detail", "--pid", "0"])).is_err());
        assert!(parse_argv(&argv(&["process-detail", "--pid", "-1"])).is_err());
    }

    #[test]
    fn rejects_equals_form_and_short_flags() {
        // 介面刻意只接受 `--key value`，越窄越安全
        assert!(parse_argv(&argv(&["process-detail", "--pid=42"])).is_err());
        assert!(parse_argv(&argv(&["process-detail", "-p", "42"])).is_err());
        assert!(parse_argv(&argv(&["process-detail", "42"])).is_err());
    }

    #[test]
    fn rejects_duplicate_flags() {
        assert!(
            parse_argv(&argv(&["process-detail", "--pid", "1", "--pid", "2"])).is_err(),
            "同一參數給兩次是可疑的注入嘗試"
        );
    }

    #[test]
    fn rejects_dangling_flag_without_value() {
        assert!(parse_argv(&argv(&["process-detail", "--pid"])).is_err());
    }

    #[test]
    fn renice_range_is_enforced_at_parse_and_validate() {
        let op = parse_argv(&argv(&[
            "renice",
            "--pid",
            "10",
            "--starttime",
            "5",
            "--nice",
            "-5",
        ]))
        .unwrap();
        assert!(op.validate().is_err(), "不開放提高優先度");
        let op = parse_argv(&argv(&[
            "renice",
            "--pid",
            "10",
            "--starttime",
            "5",
            "--nice",
            "10",
        ]))
        .unwrap();
        assert!(op.validate().is_ok());
    }

    #[test]
    fn mutations_without_starttime_are_rejected() {
        assert!(
            parse_argv(&argv(&[
                "process-signal",
                "--pid",
                "10",
                "--signal",
                "TERM"
            ]))
            .is_err(),
            "缺 starttime 就無法防 PID 重用"
        );
    }

    #[test]
    fn process_detail_never_exposes_environ() {
        let pid = std::process::id() as i32;
        let v = process_detail(pid).unwrap();
        assert!(
            v["environ"].is_null(),
            "環境變數常含 token/密碼，絕不可外洩"
        );
        assert!(v["environ_note"].is_string());
        // 但其他有用的欄位要在
        assert_eq!(v["pid"].as_i64(), Some(pid as i64));
        assert!(v["open_fds"].as_u64().is_some());
    }

    #[test]
    fn confirm_target_detects_pid_reuse() {
        let pid = std::process::id() as i32;
        // 用一個明顯錯誤的 starttime 模擬 PID 被重用
        let r = confirm_target(pid, 999_999_999);
        assert!(r.is_err(), "starttime 不符時必須拒絕");
        assert!(r.unwrap_err().to_string().contains("重用"));
    }

    #[test]
    fn confirm_target_accepts_matching_starttime() {
        let pid = std::process::id() as i32;
        let stat = util::read_string(format!("/proc/{pid}/stat")).unwrap();
        let st = sysview::collectors::process::parse_proc_stat(&stat).unwrap();
        assert!(confirm_target(pid, st.starttime).is_ok());
    }

    #[test]
    fn real_users_excludes_system_accounts() {
        for u in real_users() {
            assert!(
                (1000..65534).contains(&u.uid),
                "{} (uid {}) 是系統帳號",
                u.name,
                u.uid
            );
            assert_ne!(u.home.to_string_lossy(), "/", "家目錄不該是根目錄");
        }
    }

    #[test]
    fn lookup_user_rejects_traversal() {
        assert!(lookup_user("../root").is_none());
        assert!(lookup_user("/etc/passwd").is_none());
        assert!(lookup_user("").is_none());
    }
}
