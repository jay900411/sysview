//! 動態 entity 的說明。
//!
//! 這裡處理的是**名字會隨機器改變**的東西：`ens192`、`RTX 4070 Ti`、
//! `PID 18423`、`/home/alice/datasets`。
//!
//! 做法是：固定寫死「這**種**東西是什麼」，再把這台機器上的實際 metadata
//! 填進去。名字是 runtime 值，型別知識才是編譯期就確定的部分。
//!
//! # 不猜語意
//!
//! `/home/jay/project` 只會說「這是一個目錄，擁有者是 jay，在 / 這個檔案系統上」。
//! **不會**說「這是開發專案」—— 作業系統沒告訴我們的事就不說。
//! 同理，`python train.py` 只會說「這個行程的命令列是 python train.py」，
//! 不會說「這是在訓練模型」。
//!
//! # 不讀 environ
//!
//! Describe 絕不讀 `/proc/<pid>/environ`。那裡面幾乎一定有 token 與密碼，
//! 而且原本的機密性模型就明確排除了它 —— 不能因為多了一個說明功能就破功。

use crate::collectors::gpu::Vendor;
use crate::collectors::util;
use crate::collectors::SystemState;
use crate::metrics::describe::{Certainty, DescribeContent, EntityRef, Field};
use crate::ui::format;

pub fn describe(e: &EntityRef, s: &SystemState) -> DescribeContent {
    match e {
        EntityRef::Process { pid, starttime } => process(*pid, *starttime, s),
        EntityRef::NetworkInterface(name) => interface(name, s),
        EntityRef::Gpu(id) => gpu(id, s),
        EntityRef::Mount(path) => mount(path, s),
        EntityRef::Disk(dev) => disk(dev, s),
        EntityRef::User(uid) => user(*uid, s),
        EntityRef::Path(p) => path(p, s),
        EntityRef::CpuCore(i) => cpu_core(*i, s),
    }
}

fn base(title: impl Into<String>, e: &EntityRef) -> DescribeContent {
    DescribeContent {
        title: title.into(),
        type_name: e.type_name(),
        ..Default::default()
    }
}

// ─────────────────────────────────────────────────────────────────────────────

fn process(pid: i32, starttime: u64, s: &SystemState) -> DescribeContent {
    let e = EntityRef::Process { pid, starttime };
    let found = s
        .process
        .state()
        .processes
        .iter()
        .find(|p| p.pid == pid && p.starttime == starttime);

    let Some(p) = found else {
        let mut c = base(format!("PID {pid}"), &e);
        c.summary = "這個行程在上次取樣之後已經結束了，或者 PID 已經被別的行程重用。".to_owned();
        c.current = vec![Field::unavailable("狀態", "已消失")];
        c.source = vec![format!("/proc/{pid}/")];
        c.pitfalls =
            vec!["PID 會被重用，所以 sysview 用 (pid, starttime) 一起識別行程。".to_owned()];
        c.commands = vec![format!("ps -p {pid} -o pid,ppid,user,stat,comm")];
        return c;
    };

    let mut c = base(format!("{} (PID {})", p.name, p.pid), &e);
    c.summary =
        "這是一個目前由 Linux scheduler 管理的行程。以下是這台機器上它現在的狀態。".to_owned();
    c.current = vec![
        Field::exact("PID", p.pid.to_string()),
        Field::exact("PPID（父行程）", p.ppid.to_string()),
        Field::exact("使用者", format!("{} (uid {})", p.user, p.uid)),
        Field::exact("狀態", format!("{} — {}", p.state, p.state_label())),
        Field::derived("CPU", format!("{:.1}%", p.cpu_percent)),
        Field::exact("RSS（常駐記憶體）", format::bytes(p.rss as f64)),
        Field::exact("VIRT（位址空間）", format::bytes(p.vsize as f64)),
        Field::exact("執行緒", p.threads.to_string()),
        Field::exact("nice", p.nice.to_string()),
        Field::exact("累計 CPU 時間", format::duration(p.cpu_time_secs)),
        Field::exact("命令列", p.cmdline.clone()),
    ];
    c.source = vec![
        format!("/proc/{pid}/stat（狀態、CPU 時間、RSS、執行緒數）"),
        format!("/proc/{pid}/cmdline（命令列）"),
        format!("/proc/{pid} 的擁有者（使用者）"),
    ];
    c.how_obtained =
        Some("CPU% 是兩次取樣之間 utime+stime 的差，除以經過時間；其餘欄位是直接讀取。".to_owned());
    c.pitfalls = vec![
        "CPU% 可以超過 100% —— 多執行緒行程用了幾顆核心就是幾倍。".to_owned(),
        "RSS 會把共享函式庫在每個行程都算一次，所以全部加總會超過實體記憶體。".to_owned(),
        "VIRT 幾乎沒有參考價值：mmap 一個大檔案就會讓它變得很大，但實體用量是 0。".to_owned(),
        "sysview 刻意不讀 /proc/<pid>/environ —— 環境變數裡常有 token 與密碼。".to_owned(),
    ];
    c.commands = vec![
        format!("ps -p {pid} -o pid,ppid,user,stat,%cpu,rss,nlwp,args"),
        format!("cat /proc/{pid}/status"),
        format!("pidstat -p {pid} 1"),
    ];
    c.related = vec![
        "proc.cpu".into(),
        "proc.rss".into(),
        "proc.state".into(),
        "proc.threads".into(),
    ];
    if p.uid != util::real_uid() {
        c.permission_note = Some(
            "這是別的使用者的行程。更深入的資訊（cgroup、affinity、開啟的 fd）\
             需要管理員權限，到 Admin 頁按 u 解鎖。"
                .to_owned(),
        );
    }
    c
}

// ─────────────────────────────────────────────────────────────────────────────

fn interface(name: &str, s: &SystemState) -> DescribeContent {
    let e = EntityRef::NetworkInterface(name.to_owned());
    let mut c = base(name, &e);
    c.summary = "這是 Linux kernel 目前註冊的一個網路介面。".to_owned();

    let Some(i) = s.network.state().interfaces.iter().find(|i| i.name == name) else {
        c.current = vec![Field::unavailable("狀態", "這個介面已經不存在了")];
        c.source = vec![format!("/sys/class/net/{name}/")];
        return c;
    };

    let mut cur = vec![
        Field::exact("狀態", if i.up { "UP" } else { "DOWN" }),
        Field::exact(
            "種類",
            if i.virtual_iface {
                "虛擬介面（loopback / bridge / veth / docker / tun）"
            } else {
                "實體介面"
            },
        ),
    ];
    match &i.mac {
        Some(m) => cur.push(Field::exact("MAC", m.clone())),
        None => cur.push(Field::unavailable("MAC", "這種介面沒有 MAC 位址")),
    }
    if i.addrs.is_empty() {
        cur.push(Field::unavailable("IP", "目前沒有指派位址"));
    } else {
        for a in &i.addrs {
            cur.push(Field::exact(
                if a.contains(':') { "IPv6" } else { "IPv4" },
                a.clone(),
            ));
        }
    }
    match i.speed_mbit {
        Some(sp) => cur.push(Field::exact("連線速度", format!("{sp} Mb/s"))),
        None => cur.push(Field::unavailable(
            "連線速度",
            "驅動沒有提供（虛擬介面或未連線時很常見）",
        )),
    }
    cur.push(crate::metrics::describe::current::field(
        "目前下載",
        &i.rx_rate,
    ));
    cur.push(crate::metrics::describe::current::field(
        "目前上傳",
        &i.tx_rate,
    ));
    cur.push(Field::exact(
        "開機以來累計",
        format!(
            "收 {} / 送 {}",
            format::bytes(i.rx_total as f64),
            format::bytes(i.tx_total as f64)
        ),
    ));
    cur.push(Field::exact(
        "錯誤 / 丟包",
        format!("{} / {}", i.errors, i.drops),
    ));
    c.current = cur;

    c.source = vec![
        "/proc/net/dev（收送位元組、封包、錯誤、丟包）".to_owned(),
        format!("/sys/class/net/{name}/operstate（UP/DOWN）"),
        format!("/sys/class/net/{name}/address（MAC）"),
        format!("/sys/class/net/{name}/speed（連線速度）"),
        "getifaddrs(3)（IP 位址）".to_owned(),
    ];
    c.how_obtained = Some(
        "速率是兩次取樣之間位元組累計值的差，除以經過時間。\
         IP 用 getifaddrs(3) 直接取得，不 fork `ip` 指令。"
            .to_owned(),
    );
    c.pitfalls = vec![
        "速率的單位是 bytes/s。一般講的 Mbps 要 ×8 ÷ 1e6 —— 千兆網路上限約 125 MB/s。".to_owned(),
        "loopback 的流量會同時算進收和送，所以 sysview 不把它計入總量。".to_owned(),
        "drop 持續增加通常是接收佇列滿了；errs 增加多半是實體層問題。".to_owned(),
    ];
    c.commands = vec![
        format!("ip addr show {name}"),
        format!("ip -s link show {name}"),
        format!("ethtool {name}"),
    ];
    c.related = vec![
        "net.throughput".into(),
        "net.errors".into(),
        "net.sockets".into(),
    ];
    c
}

// ─────────────────────────────────────────────────────────────────────────────

fn gpu(id: &str, s: &SystemState) -> DescribeContent {
    let e = EntityRef::Gpu(id.to_owned());
    let Some(d) = s.gpu.state().by_id(id) else {
        let mut c = base(id, &e);
        c.summary = "這個 GPU 裝置已經不在了。".to_owned();
        return c;
    };
    let mut c = base(d.name.clone(), &e);
    c.summary = "這是一個由作業系統與顯示驅動管理的圖形／平行運算裝置。".to_owned();

    let mut cur = vec![
        Field::exact("廠商", d.vendor.label()),
        Field::exact("型號", d.name.clone()),
        Field::exact("sysview 內部 id", d.id.clone()),
    ];
    if let Some(v) = &d.driver_version {
        cur.push(Field::exact("驅動版本", v.clone()));
    }
    // 使用率的確定性直接反映 backend
    let util_field = {
        let mut f = crate::metrics::describe::current::field("使用率", &d.utilization);
        if d.utilization.quality.is_estimated() {
            f.certainty = Certainty::Estimated;
        }
        f
    };
    cur.push(util_field);
    if d.memory_total.get().is_some() {
        cur.push(Field::exact(
            "VRAM",
            format!(
                "{} / {}",
                format::reading(&d.memory_used),
                format::reading(&d.memory_total)
            ),
        ));
    } else {
        cur.push(Field::unavailable(
            "VRAM",
            "內顯與系統記憶體共用，DRM 沒有獨立的 VRAM 介面",
        ));
    }
    for (label, r) in [
        ("溫度", &d.temperature),
        ("功耗", &d.power),
        ("核心時脈", &d.sm_clock),
        ("記憶體時脈", &d.mem_clock),
        ("風扇", &d.fan),
    ] {
        cur.push(crate::metrics::describe::current::field(label, r));
    }
    if let Some(p) = &d.pstate {
        cur.push(Field::exact("效能狀態", p.clone()));
    }
    if let Some(r) = &d.clock_range {
        cur.push(Field::exact("時脈範圍", r.clone()));
    }
    if let (Some(g), Some(w)) = (d.pcie_gen, d.pcie_width) {
        cur.push(Field::exact("PCIe", format!("gen{g} x{w}")));
    }
    cur.push(Field::exact(
        "compute 行程",
        if d.processes.is_empty() {
            "無".to_owned()
        } else {
            format!("{} 個", d.processes.len())
        },
    ));
    c.current = cur;

    // Source 直接反映實際用到的 backend，不是通則
    c.source = vec![d.backend.to_owned()];
    c.how_obtained = Some(match d.vendor {
        Vendor::Nvidia => "透過 NVML（libnvidia-ml.so，執行期 dlopen）直接向驅動查詢。\
             nvidia-smi 自己也是建在同一套 NVML 上，但 sysview 不 fork 它 —— \
             每次啟動 nvidia-smi 都要重新初始化驅動，實測貴 9 倍。"
            .to_owned(),
        Vendor::Intel => "i915 沒有 busy% 介面。使用率是從 power/rc6_residency_ms（GPU 進入省電\
             休眠的累計時間）反推：這段期間「沒在睡」的比例約等於忙碌比例。\
             因此標示為 estimated。"
            .to_owned(),
        Vendor::Amd => {
            "從 amdgpu 的 sysfs 讀取。gpu_busy_percent 是驅動直接提供的精確值。".to_owned()
        }
    });
    c.pitfalls = match d.vendor {
        Vendor::Nvidia => vec![
            "使用率是「取樣期間至少有一個 kernel 在跑」的時間比例，\
             不等於 CUDA core 佔用率 —— 100% 不代表算力用滿。"
                .to_owned(),
            "Tensor Core 的使用率完全不在這個數字裡。".to_owned(),
            "VRAM 的 used 是框架（PyTorch/TF）抓走的量，不是張量實際佔用的量。".to_owned(),
            "閒置時降到 P8 與 PCIe gen1 是正常省電行為。".to_owned(),
        ],
        Vendor::Intel => vec![
            "使用率是估計值，不是驅動提供的精確數字。".to_owned(),
            "RC6 只反映有沒有進入休眠，不區分是在算圖還是只是被喚醒著。".to_owned(),
            "要精確數字請用 intel_gpu_top，但它需要 root 或 CAP_PERFMON。".to_owned(),
        ],
        Vendor::Amd => vec![
            "這個 backend 依 amdgpu sysfs ABI 實作，但開發機上沒有 AMD 卡，\
             未經實機驗證。"
                .to_owned(),
        ],
    };
    c.commands = match d.vendor {
        Vendor::Nvidia => vec![
            "nvidia-smi".to_owned(),
            "nvidia-smi -q".to_owned(),
            "nvidia-smi dmon".to_owned(),
            "nvtop".to_owned(),
        ],
        Vendor::Intel => vec![
            format!("cat /sys/class/drm/{id}/gt_act_freq_mhz"),
            format!("cat /sys/class/drm/{id}/power/rc6_residency_ms"),
            "sudo intel_gpu_top".to_owned(),
        ],
        Vendor::Amd => vec![
            format!("cat /sys/class/drm/{id}/device/gpu_busy_percent"),
            "radeontop".to_owned(),
        ],
    };
    c.commands.push("lspci -nn | grep -Ei 'vga|3d'".to_owned());
    c.related = vec![
        "gpu.util".into(),
        "gpu.vram".into(),
        "gpu.power".into(),
        "gpu.process".into(),
    ];
    if !d.process_list_complete && !d.processes.is_empty() {
        c.permission_note = Some(
            "compute 行程清單只包含你自己的行程。要看所有使用者的，\
             到 Admin 頁按 u 解鎖。"
                .to_owned(),
        );
    }
    c
}

// ─────────────────────────────────────────────────────────────────────────────

fn mount(point: &str, s: &SystemState) -> DescribeContent {
    let e = EntityRef::Mount(point.to_owned());
    let Some(m) = s
        .disk
        .state()
        .mounts
        .iter()
        .find(|m| m.mount_point == point)
    else {
        let mut c = base(point, &e);
        c.summary = "這個掛載點已經不在了（可能剛被卸載）。".to_owned();
        return c;
    };
    let mut c = base(&m.mount_point, &e);
    c.summary = "這是一個目前掛載中的檔案系統。以下是它在這台機器上的容量狀況。".to_owned();
    c.current = vec![
        Field::exact("掛載點", m.mount_point.clone()),
        Field::exact("裝置", m.device.clone()),
        Field::exact("檔案系統", m.fs_type.clone()),
        Field::exact("總容量", format::bytes(m.total as f64)),
        Field::exact("已用", format::bytes(m.used as f64)),
        Field::exact("可用", format::bytes(m.available as f64)),
        crate::metrics::describe::current::field("使用率", &m.usage),
        crate::metrics::describe::current::field("inode 使用率", &m.inode_usage),
    ];
    c.source = vec![
        "/proc/mounts（掛載點、裝置、檔案系統類型）".to_owned(),
        "statvfs(2)（容量與 inode）".to_owned(),
    ];
    c.derivation = Some("使用率 = used ÷ (used + available)，與 df 的 Use% 定義相同。".to_owned());
    c.how_obtained = Some(
        "掛載點每 5 秒重新掃一次（每個掛載點一次 statvfs，對掛掉的 NFS 會阻塞，\
         所以不跟著主更新率走）。"
            .to_owned(),
    );
    c.pitfalls = vec![
        "ext4 預設保留 5% 給 root。用 used/total 算會比 df 少報幾個百分點，\
         所以 sysview 採用 df 的定義。"
            .to_owned(),
        "刪掉但仍被行程開啟的檔案不會釋放空間（用 lsof +L1 找）。".to_owned(),
        "inode 用完會出現 No space left on device，即使 df 顯示還有空間。".to_owned(),
    ];
    c.commands = vec![
        format!("df -h {}", m.mount_point),
        format!("df -i {}", m.mount_point),
        format!("du -xh --max-depth=1 {}", m.mount_point),
        "lsof +L1".to_owned(),
    ];
    c.related = vec![
        "disk.usage".into(),
        "disk.inodes".into(),
        "storage.user".into(),
    ];
    c
}

// ─────────────────────────────────────────────────────────────────────────────

fn disk(name: &str, s: &SystemState) -> DescribeContent {
    let e = EntityRef::Disk(name.to_owned());
    let Some(d) = s.disk.state().devices.iter().find(|d| d.name == name) else {
        let mut c = base(name, &e);
        c.summary = "這個區塊裝置已經不在了。".to_owned();
        return c;
    };
    let mut c = base(format!("{} · {}", d.name, d.model), &e);
    c.summary = "這是一個區塊裝置（實體磁碟）。以下是它目前的 I/O 狀況。".to_owned();
    let mut cur = vec![
        Field::exact("核心裝置名稱", d.name.clone()),
        Field::exact("型號", d.model.clone()),
        Field::exact(
            "種類",
            match d.rotational {
                Some(true) => "HDD（機械硬碟）",
                Some(false) => "SSD / NVMe",
                None => "未知",
            },
        ),
    ];
    match d.size {
        Some(sz) => cur.push(Field::exact("容量", format::bytes(sz as f64))),
        None => cur.push(Field::unavailable("容量", "讀不到 /sys/block/.../size")),
    }
    for (l, r) in [
        ("讀取速率", &d.read_rate),
        ("寫入速率", &d.write_rate),
        ("IOPS", &d.iops),
        ("忙碌率", &d.util),
        ("溫度", &d.temp),
    ] {
        cur.push(crate::metrics::describe::current::field(l, r));
    }
    c.current = cur;
    c.source = vec![
        "/proc/diskstats（讀寫 sector、完成次數、io_ticks）".to_owned(),
        format!("/sys/block/{name}/device/model（型號）"),
        format!("/sys/block/{name}/queue/rotational（HDD 或 SSD）"),
        format!("/sys/block/{name}/size（容量，單位 512 位元組）"),
        "/sys/class/hwmon/*（NVMe 溫度）".to_owned(),
    ];
    c.derivation =
        Some("吞吐量 = Δsector × 512 ÷ Δt；忙碌率 = Δio_ticks(ms) ÷ Δt(ms) × 100。".to_owned());
    c.pitfalls = vec![
        "diskstats 的 sector 固定是 512 位元組，與實體 sector 大小無關。".to_owned(),
        "SSD/NVMe 能同時處理數十個請求，所以忙碌率 100% 不代表已達極限；\
         機械硬碟則相當可靠。"
            .to_owned(),
        "sysview 只統計整顆碟，不重複計分割區，否則吞吐量會加倍。".to_owned(),
    ];
    c.commands = vec![
        "iostat -xz 1".to_owned(),
        format!("lsblk -o NAME,SIZE,MODEL,ROTA {}", format!("/dev/{name}")),
        format!("smartctl -a /dev/{name}"),
    ];
    c.related = vec![
        "disk.throughput".into(),
        "disk.iops".into(),
        "disk.util".into(),
    ];
    c
}

// ─────────────────────────────────────────────────────────────────────────────

fn user(uid: u32, s: &SystemState) -> DescribeContent {
    let e = EntityRef::User(uid);
    let name = util::username(uid);
    let mut c = base(name.clone(), &e);
    c.summary = "這是這台機器上的一個系統帳號。".to_owned();

    let procs: Vec<_> = s
        .process
        .state()
        .processes
        .iter()
        .filter(|p| p.uid == uid)
        .collect();
    let rss: u64 = procs.iter().map(|p| p.rss).sum();
    let cpu: f64 = procs.iter().map(|p| p.cpu_percent).sum();

    c.current = vec![
        Field::exact("使用者名稱", name),
        Field::exact("uid", uid.to_string()),
        Field::derived("你看得到的行程數", procs.len().to_string()),
        Field::derived("這些行程的 RSS 總和", format::bytes(rss as f64)),
        Field::derived("這些行程的 CPU 總和", format!("{cpu:.1}%")),
    ];
    c.source = vec![
        "/proc/<pid> 的擁有者".to_owned(),
        "getpwuid(3)（uid → 使用者名稱）".to_owned(),
    ];
    c.how_obtained =
        Some("這裡的統計只涵蓋**你有權限看到的行程**，不是這個使用者的全部。".to_owned());
    c.pitfalls = vec![
        "RSS 總和會重複計算共享函式庫，不能當成這個使用者真正佔用的記憶體。\
         公平的算法是 PSS，那需要讀 smaps_rollup，一般使用者只能讀自己的。"
            .to_owned(),
    ];
    c.commands = vec![
        format!("ps -u {uid} -o pid,user,%cpu,rss,args"),
        format!("id {uid}"),
        format!("getent passwd {uid}"),
    ];
    c.related = vec!["proc.rss".into(), "storage.user".into()];
    c.permission_note = Some(
        "完整的跨使用者統計（含 PSS、家目錄用量、GPU VRAM）需要管理員權限，\
         到 Admin 頁按 u 解鎖。"
            .to_owned(),
    );
    c
}

// ─────────────────────────────────────────────────────────────────────────────

fn path(p: &str, s: &SystemState) -> DescribeContent {
    let e = EntityRef::Path(p.to_owned());
    let mut c = base(p, &e);
    // 刻意只講「這是什麼類型的系統物件」，不猜它的用途
    c.summary = "這是檔案系統上的一個路徑。sysview 只回報作業系統提供的中繼資料，\
                 不推測這個目錄的用途。"
        .to_owned();

    let mut cur = vec![Field::exact("路徑", p.to_owned())];
    match std::fs::symlink_metadata(p) {
        Ok(md) => {
            use std::os::unix::fs::MetadataExt;
            let ft = md.file_type();
            cur.push(Field::exact(
                "型別",
                if ft.is_dir() {
                    "目錄"
                } else if ft.is_symlink() {
                    "符號連結"
                } else if ft.is_file() {
                    "一般檔案"
                } else {
                    "特殊檔案（socket / fifo / 裝置）"
                },
            ));
            cur.push(Field::exact("擁有者", util::username(md.uid())));
            cur.push(Field::exact("權限", format!("{:o}", md.mode() & 0o7777)));
            if !ft.is_dir() {
                cur.push(Field::exact("大小", format::bytes(md.size() as f64)));
            }
        }
        Err(err) => {
            cur.push(Field::unavailable(
                "中繼資料",
                match err.kind() {
                    std::io::ErrorKind::PermissionDenied => "沒有權限讀取（這是正確的權限行為）",
                    std::io::ErrorKind::NotFound => "路徑不存在",
                    _ => "讀取失敗",
                },
            ));
        }
    }
    // 找出這個路徑落在哪個掛載點上
    if let Some(m) = s
        .disk
        .state()
        .mounts
        .iter()
        .filter(|m| p.starts_with(&m.mount_point))
        .max_by_key(|m| m.mount_point.len())
    {
        cur.push(Field::derived("所在檔案系統", m.mount_point.clone()));
        cur.push(Field::exact("檔案系統類型", m.fs_type.clone()));
    }
    c.current = cur;
    c.source = vec![
        "lstat(2)（型別、擁有者、權限、大小）".to_owned(),
        "/proc/mounts（判斷落在哪個檔案系統）".to_owned(),
    ];
    c.pitfalls = vec![
        "sysview 只讀中繼資料，不開啟檔案內容。".to_owned(),
        "目錄的用量要另外遞迴走訪才知道，那是按需執行的動作。".to_owned(),
    ];
    c.commands = vec![
        format!("ls -ld {p}"),
        format!("stat {p}"),
        format!("du -xsh {p}"),
        format!("df -h {p}"),
    ];
    c.related = vec!["disk.usage".into(), "storage.user".into()];
    c
}

// ─────────────────────────────────────────────────────────────────────────────

fn cpu_core(i: usize, s: &SystemState) -> DescribeContent {
    let e = EntityRef::CpuCore(i);
    let cpu = s.cpu.state();
    let mut c = base(format!("CPU {i}"), &e);
    c.summary =
        "這是一顆邏輯處理器。開了超執行緒的話，兩顆邏輯處理器會共用一個實體核心。".to_owned();
    let Some(core) = cpu.cores.get(i) else {
        c.current = vec![Field::unavailable("狀態", "這顆核心不存在")];
        return c;
    };
    let mut cur = vec![
        Field::exact("編號", i.to_string()),
        crate::metrics::describe::current::field("使用率", &core.usage),
        crate::metrics::describe::current::field("目前頻率", &core.freq_mhz),
    ];
    if let Some(t) = cpu.core_temp(i) {
        cur.push(Field::exact("溫度", format!("{t:.0}°C")));
    }
    if let Some(id) = cpu.topology.get(i) {
        // 溫度是實體核心的，講清楚這顆邏輯處理器坐在哪，數字才不會看起來像在騙人
        let siblings: Vec<String> = (0..cpu.logical)
            .filter(|&j| j != i && cpu.topology.get(j) == Some(id))
            .map(|j| format!("CPU {j}"))
            .collect();
        cur.push(Field::exact("實體核心", format!("core {}", id.core)));
        if !siblings.is_empty() {
            cur.push(Field::exact("共用這顆核心的", siblings.join("、")));
        }
    }
    c.current = cur;
    c.source = vec![
        format!("/proc/stat 的 cpu{i} 那一行"),
        format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq"),
        "/sys/class/hwmon/*（coretemp）".to_owned(),
    ];
    c.derivation =
        Some("使用率 = 1 − (Δidle + Δiowait) ÷ Δ全部欄位，取兩次取樣之間的差。".to_owned());
    c.pitfalls = vec![
        "邏輯處理器不等於實體核心。超執行緒下兩顆邏輯處理器跑滿 ≠ 兩倍算力。".to_owned(),
        "閒置時降到最低頻率是正常省電行為。".to_owned(),
        "排程器會搬移工作，所以單一核心的使用率跳動很正常。".to_owned(),
    ];
    c.commands = vec![
        "mpstat -P ALL 1".to_owned(),
        format!("cat /sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq"),
        "lscpu -e".to_owned(),
    ];
    c.related = vec!["cpu.usage".into(), "cpu.freq".into(), "cpu.temp".into()];
    c
}
