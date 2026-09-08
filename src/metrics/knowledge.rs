//! Knowledge layer — sysview 與 btop / bottom 最大的差異。
//!
//! v1 把「load average 是什麼」「iowait 高代表什麼」這類提示直接寫死在 UI 字串裡。
//! v2 把它們變成**結構化的 metric 定義**：每個數字都能回答
//!
//! * What is this?      這個數字代表什麼
//! * Source             Linux 從哪裡拿到的
//! * Formula            怎麼算出來的
//! * Pitfalls           容易誤解的地方
//! * Commands           對應的原生指令（讓使用者學得會，而不是只會看 sysview）
//! * Related            該一起看的其他 metric
//!
//! UI 在任何 metric 上按 `e` 就會叫出 [`crate::ui::explain`] 面板。

use crate::metrics::model::Unit;

/// 一個 metric 的完整說明。全部是 `&'static str`，編進 binary，不需要外部檔案。
#[derive(Clone, Copy, Debug)]
pub struct MetricDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub unit: Unit,
    /// 一句話講清楚這是什麼。
    pub meaning: &'static str,
    /// 資料來源：檔案路徑、核心介面或函式庫。
    pub sources: &'static [&'static str],
    /// 計算方式。看得懂的算式，不是程式碼。
    pub formula: &'static str,
    /// 容易誤解的地方。這是最有價值的一欄。
    pub pitfalls: &'static [&'static str],
    /// 對應的原生指令，讓使用者可以自己驗證、也學得到工具。
    pub commands: &'static [&'static str],
    /// 判斷瓶頸時該一起看的 metric id。
    pub related: &'static [&'static str],
}

/// 全部 metric 定義。以 id 排序，[`lookup`] 用二分搜尋。
pub static METRICS: &[MetricDefinition] = &[
    // ⚠ 必須依 id 遞增排序，lookup() 靠二分搜尋。
    // metrics_are_sorted_for_binary_search 測試會強制檢查。
    MetricDefinition {
        id: "cgroup.cpu",
        title: "cgroup CPU Limit",
        unit: Unit::Percent,
        meaning: "目前 cgroup（容器）被允許使用的 CPU 上限，換算成等效核心數。",
        sources: &["/sys/fs/cgroup/cpu.max", "/proc/self/cgroup"],
        formula: "quota ÷ period，例如 200000/100000 = 2 顆核心",
        pitfalls: &[
            "在容器裡 /proc/stat 顯示的是宿主機的全部核心，不是你的配額。",
            "cpu.max 是 \"max\" 代表沒有上限。",
            "被 throttle 的次數要看 cpu.stat 的 nr_throttled，那才是真的被限速。",
        ],
        commands: &["cat /sys/fs/cgroup/cpu.max", "cat /sys/fs/cgroup/cpu.stat"],
        related: &["cpu.usage", "cgroup.memory"],
    },
    MetricDefinition {
        id: "cgroup.memory",
        title: "cgroup Memory Limit",
        unit: Unit::Bytes,
        meaning: "目前 cgroup（容器）的記憶體上限與實際用量。",
        sources: &["/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory.current"],
        formula: "直接讀取；\"max\" 代表無上限",
        pitfalls: &[
            "容器裡 free / /proc/meminfo 顯示的是宿主機記憶體，不是你的配額。",
            "超過 memory.max 會直接被 OOM killer 殺掉，不會像 swap 一樣慢慢變慢。",
        ],
        commands: &["cat /sys/fs/cgroup/memory.max", "cat /sys/fs/cgroup/memory.current"],
        related: &["mem.used", "cgroup.cpu"],
    },
    MetricDefinition {
        id: "cpu.ctxt",
        title: "Context Switches",
        unit: Unit::CountPerSec,
        meaning: "核心每秒把 CPU 從一個執行緒切換到另一個執行緒的次數。",
        sources: &["/proc/stat (ctxt 欄)"],
        formula: "(ctxt 現在 − ctxt 上次) ÷ 經過秒數",
        pitfalls: &[
            "沒有「正常值」。幾千到幾十萬都可能正常，要看工作負載型態。",
            "突然暴增通常代表 lock 競爭、忙碌等待，或執行緒開太多。",
            "這是全系統累計值，無法直接歸咎到某個行程。",
        ],
        commands: &["vmstat 1", "pidstat -w 1"],
        related: &["cpu.usage", "cpu.running_blocked"],
    },
    MetricDefinition {
        id: "cpu.forks",
        title: "Process Creation Rate",
        unit: Unit::CountPerSec,
        meaning: "每秒新建立的行程數（fork + clone）。",
        sources: &["/proc/stat (processes 欄)"],
        formula: "(processes 現在 − processes 上次) ÷ 經過秒數",
        pitfalls: &[
            "shell script 迴圈裡每圈開一個外部指令，這個數字會很高但 CPU 使用率不高。",
            "數值持續偏高常代表有人在用 shell 迴圈做本來該批次處理的事。",
        ],
        commands: &["vmstat 1", "execsnoop-bpfcc"],
        related: &["cpu.ctxt", "proc.count"],
    },
    MetricDefinition {
        id: "cpu.freq",
        title: "CPU Frequency",
        unit: Unit::Megahertz,
        meaning: "各邏輯核心目前的時脈。現代 CPU 會依負載、溫度與功耗上限動態調整。",
        sources: &[
            "/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq",
            "/proc/cpuinfo (cpu MHz，備援)",
        ],
        formula: "直接讀取，單位由 kHz 換算為 MHz",
        pitfalls: &[
            "閒置時降到 800 MHz 是正常省電行為，不是故障。",
            "scaling_cur_freq 是核心「認為」的頻率；實測值要看 aperf/mperf。",
            "全核心同時滿載時，達不到單核 turbo 上限是正常的。",
            "虛擬機裡這個值常常是假的或固定不動。",
        ],
        commands: &["watch -n1 'grep MHz /proc/cpuinfo'", "turbostat", "cpupower frequency-info"],
        related: &["cpu.temp", "cpu.usage"],
    },
    MetricDefinition {
        id: "cpu.intr",
        title: "Interrupts",
        unit: Unit::CountPerSec,
        meaning: "每秒硬體中斷次數 —— 裝置（網卡、磁碟、計時器）打斷 CPU 要求處理的次數。",
        sources: &["/proc/stat 的 intr 那一行（第一個數字是總計）"],
        formula: "(intr 累計值的差) ÷ 經過秒數",
        pitfalls: &[
            "數值高本身不是問題。高吞吐量的網路與 NVMe 本來就會產生大量中斷。",
            "值得注意的是「突然變化」而不是絕對值 —— 那通常代表裝置或驅動出了狀況。",
            "現代網卡會做中斷聚合（interrupt coalescing），所以中斷數遠少於封包數。",
            "這裡只算硬體中斷。軟體中斷（softirq）要看 /proc/softirqs，不在這個數字裡。",
            "要知道是哪個裝置在中斷，得看 /proc/interrupts —— /proc/stat 只給總數。",
        ],
        commands: &["cat /proc/interrupts", "vmstat 1", "mpstat -I SUM 1", "watch -d cat /proc/interrupts"],
        related: &["cpu.ctxt", "cpu.usage", "disk.iops"],
    },
    MetricDefinition {
        id: "cpu.iowait",
        title: "CPU I/O Wait",
        unit: Unit::Percent,
        meaning: "CPU 閒著、但至少有一個行程正在等磁碟或網路 I/O 完成的時間比例。",
        sources: &["/proc/stat (第 5 欄 iowait)"],
        formula: "(iowait 差值 ÷ 全部 CPU 時間差值) × 100",
        pitfalls: &[
            "iowait 是「閒置」的一種，不是 CPU 在忙。它算在 idle 裡面。",
            "iowait 低不代表沒有 I/O 問題 —— CPU 忙的時候 I/O 等待會被算成別的狀態。",
            "多核機器上這個值會被稀釋：一個核心完全卡在 I/O，24 核平均下來只有 4%。",
            "要判斷磁碟是不是瓶頸，看 disk.util 和 disk.await 比看 iowait 可靠。",
        ],
        commands: &["iostat -xz 1", "mpstat -P ALL 1", "vmstat 1"],
        related: &["disk.util", "disk.throughput", "cpu.running_blocked"],
    },
    MetricDefinition {
        id: "cpu.load",
        title: "Load Average",
        unit: Unit::Scalar,
        meaning: "過去 1 / 5 / 15 分鐘內，平均有多少行程處於「可執行」或「不可中斷等待」狀態。",
        sources: &["/proc/loadavg"],
        formula: "核心以指數移動平均計算，樣本每 5 秒取一次",
        pitfalls: &[
            "Linux 的 load 和 Unix 傳統定義不同：它把 D 狀態（等磁碟/NFS）也算進去，所以 I/O 塞車也會推高 load。",
            "要除以核心數才有意義。24 核機器 load 12 是半載，不是超載。",
            "load 高但 CPU 使用率低 → 幾乎一定是 I/O 或 lock 卡住，不是算不完。",
            "1 分鐘值抖動很大，趨勢要看 5 / 15 分鐘值。",
        ],
        commands: &["uptime", "cat /proc/loadavg", "vmstat 1"],
        related: &["cpu.usage", "cpu.iowait", "cpu.running_blocked"],
    },
    MetricDefinition {
        id: "cpu.load_per_core",
        title: "Load per Core",
        unit: Unit::Scalar,
        meaning: "把 1 分鐘平均負載除以邏輯處理器數，換算成「每顆核心平均排了幾個工作」。",
        sources: &["/proc/loadavg", "/proc/stat 的 cpuN 行數"],
        formula: "1 分鐘 load average ÷ 邏輯處理器數",
        pitfalls: &[
            "1.0 代表剛好排滿。超過 1.0 表示有工作在排隊等 CPU。",
            "Linux 的 load 包含不可中斷 I/O 等待（D 狀態），所以磁碟卡住也會把 load 推高，即使 CPU 很閒。",
            "分母用的是邏輯處理器數，但超執行緒的兩條執行緒不等於兩顆核心，所以這個比值偏樂觀。",
            "短暫超過 1.0 很正常，持續超過才值得查。",
        ],
        commands: &["uptime", "cat /proc/loadavg", "nproc"],
        related: &["cpu.load", "cpu.usage", "cpu.running_blocked"],
    },
    MetricDefinition {
        id: "cpu.logical",
        title: "Logical CPUs",
        unit: Unit::Count,
        meaning: "作業系統看得到、可以獨立排程的處理器數量。",
        sources: &["/proc/stat 的 cpuN 行數"],
        formula: "直接計數",
        pitfalls: &[
            "這不是實體核心數。開了超執行緒（SMT）之後，一顆實體核心會呈現成兩顆邏輯處理器。",
            "兩條執行緒跑滿一顆實體核心，效能不會是單條的兩倍 —— 它們共用執行單元與快取。",
            "混合架構（Intel P-core / E-core）的邏輯處理器效能並不相同，數量無法反映這件事。",
            "在容器裡看到的是宿主機的數量，不是 cgroup 配額 —— 配額要看 cpu.max。",
            "`nproc` 回報的是這個行程被 affinity 允許使用的數量，可能比這裡少。",
        ],
        commands: &["nproc", "lscpu", "lscpu -e", "cat /sys/devices/system/cpu/online"],
        related: &["cpu.usage", "cpu.model", "cgroup.cpu"],
    },
    MetricDefinition {
        id: "cpu.model",
        title: "CPU Model",
        unit: Unit::Scalar,
        meaning: "處理器自己回報的型號字串。",
        sources: &["/proc/cpuinfo 的 model name 欄位（來自 CPUID）"],
        formula: "直接讀取，不做加工",
        pitfalls: &[
            "這是 CPU 自己講的，不是查資料庫得來的 —— 虛擬機可以任意偽造這個字串。",
            "字串裡的 GHz 是**標稱**基礎頻率，不是現在跑多快。實際頻率要看 CPU Frequency。",
            "非 x86 平台的 /proc/cpuinfo 格式不同，可能沒有 model name 這一欄。",
        ],
        commands: &["lscpu", "cat /proc/cpuinfo | grep 'model name' | head -1"],
        related: &["cpu.logical", "cpu.freq"],
    },
    MetricDefinition {
        id: "cpu.running_blocked",
        title: "Running / Blocked Processes",
        unit: Unit::Count,
        meaning: "當下可執行的行程數，以及卡在不可中斷 I/O（D 狀態）的行程數。",
        sources: &["/proc/stat (procs_running, procs_blocked)"],
        formula: "核心直接提供的瞬時計數",
        pitfalls: &[
            "blocked 持續大於 0 是磁碟或 NFS 有問題的強烈訊號。",
            "running 遠大於核心數代表 CPU 過載，行程在排隊。",
            "這是瞬時值，抖動比 load average 大很多。",
        ],
        commands: &["vmstat 1", "ps -eo state,pid,comm | grep '^D'"],
        related: &["cpu.load", "cpu.iowait", "proc.state"],
    },
    MetricDefinition {
        id: "cpu.steal",
        title: "CPU Steal Time",
        unit: Unit::Percent,
        meaning: "在虛擬機裡，本來排給你、卻被 hypervisor 拿去給別的 VM 用掉的時間比例。",
        sources: &["/proc/stat (第 8 欄 steal)"],
        formula: "(steal 差值 ÷ 全部 CPU 時間差值) × 100",
        pitfalls: &[
            "實體機上這個值永遠是 0，只有虛擬化環境才有意義。",
            "持續超過 5–10% 代表你和別人搶同一台實體主機，這不是你的程式的問題。",
            "雲端「可爆發型」機型（如 AWS t 系列）額度用完後 steal 會飆高。",
        ],
        commands: &["vmstat 1", "mpstat -P ALL 1", "top（%st 欄）"],
        related: &["cpu.usage", "cpu.load"],
    },
    MetricDefinition {
        id: "cpu.temp",
        title: "CPU Temperature",
        unit: Unit::Celsius,
        meaning: "CPU 封裝（package）的溫度，通常也會有每個實體核心的個別溫度。",
        sources: &[
            "/sys/class/hwmon/hwmon*/temp*_input (coretemp / k10temp)",
            "/sys/class/thermal/thermal_zone*/temp (備援)",
        ],
        formula: "讀出的毫度數 ÷ 1000",
        pitfalls: &[
            "各家 CPU 的過熱門檻不同，多數 Intel 桌機在 100°C 才降頻，80°C 並不危險。",
            "筆電短時間衝到 90°C+ 是設計內行為。",
            "hwmon 的編號開機後可能改變，不要寫死 hwmon0。",
            "虛擬機通常完全讀不到溫度。",
        ],
        commands: &["sensors", "watch -n1 sensors"],
        related: &["cpu.freq", "cpu.usage"],
    },
    MetricDefinition {
        id: "cpu.usage",
        title: "CPU Utilization",
        unit: Unit::Percent,
        meaning: "取樣區間內，CPU 不在 idle 狀態的時間比例。",
        sources: &["/proc/stat (cpu 與 cpuN 各行)"],
        formula: "(1 − (idle + iowait 的差值) ÷ 全部欄位的差值) × 100",
        pitfalls: &[
            "這是兩次取樣之間的平均值，不是瞬時值。取樣間隔越長越平滑。",
            "100% 只代表 CPU 一直有事做，不代表算得有效率（可能都在等記憶體）。",
            "超執行緒讓邏輯核心數翻倍，但 24 個邏輯核心跑滿 ≠ 24 倍算力。",
            "在容器裡看到的是宿主機的使用率，不是 cgroup 配額的使用率。",
        ],
        commands: &["mpstat -P ALL 1", "top", "htop", "pidstat 1"],
        related: &["cpu.load", "cpu.freq", "cpu.iowait", "cgroup.cpu"],
    },
    MetricDefinition {
        id: "disk.inodes",
        title: "Inode Usage",
        unit: Unit::Percent,
        meaning: "檔案系統中已使用的 inode 比例。一個 inode 對應一個檔案或目錄。",
        sources: &["statvfs(2): f_files, f_ffree"],
        formula: "(f_files − f_ffree) ÷ f_files × 100",
        pitfalls: &[
            "inode 用完會出現「No space left on device」，即使 df 顯示還有很多空間。",
            "大量小檔案（node_modules、郵件佇列、快取）最容易吃光 inode。",
            "XFS / Btrfs 是動態配置 inode，這個數字意義不大。",
        ],
        commands: &["df -i", "find /path -xdev -type f | wc -l"],
        related: &["disk.usage"],
    },
    MetricDefinition {
        id: "disk.iops",
        title: "Disk IOPS",
        unit: Unit::CountPerSec,
        meaning: "每秒完成的 I/O 請求數。",
        sources: &["/proc/diskstats (第 4 + 8 欄：完成的讀寫次數)"],
        formula: "(完成次數差值) ÷ 經過秒數",
        pitfalls: &[
            "核心會合併相鄰請求，所以這裡的次數比應用層發出的次數少。",
            "機械硬碟隨機 IOPS 約 100–200；NVMe 可到數十萬。同一個數字意義完全不同。",
            "高 IOPS + 低吞吐量 = 隨機小 I/O，這對 HDD 是最壞情況。",
        ],
        commands: &["iostat -xz 1", "biolatency-bpfcc"],
        related: &["disk.throughput", "disk.util"],
    },
    MetricDefinition {
        id: "disk.throughput",
        title: "Disk Throughput",
        unit: Unit::BytesPerSec,
        meaning: "區塊裝置每秒實際讀取與寫入的位元組數。",
        sources: &["/proc/diskstats (第 6 欄讀 sector、第 10 欄寫 sector)"],
        formula: "(sector 差值 × 512) ÷ 經過秒數",
        pitfalls: &[
            "diskstats 的 sector 固定是 512 位元組，跟實體 sector 大小無關。",
            "這是區塊層的量，寫入可能還在 page cache 裡沒真的落盤。",
            "sysview 只統計整顆磁碟，不重複計分割區，否則會加倍。",
        ],
        commands: &["iostat -xz 1", "dstat -d", "biotop-bpfcc"],
        related: &["disk.iops", "disk.util", "cpu.iowait"],
    },
    MetricDefinition {
        id: "disk.usage",
        title: "Filesystem Usage",
        unit: Unit::Percent,
        meaning: "掛載點已使用的空間比例。",
        sources: &["statvfs(2): f_blocks, f_bfree, f_bavail, f_frsize"],
        formula: "used ÷ (used + available) × 100，與 df 的 Use% 定義一致",
        pitfalls: &[
            "ext4 預設保留 5% 給 root。df 的 Use% 排除這塊，所以 used/total 會比 df 少幾個百分點。",
            "刪掉的檔案若還被行程開著，空間不會釋放（用 lsof +L1 找）。",
            "snap 之類的 bind mount 會讓同一個檔案系統出現很多次，sysview 已自動去重。",
            "超過 90% 時 ext4 的配置器容易產生碎片，效能會下降。",
        ],
        commands: &["df -h", "df -i", "du -xh --max-depth=1 /path", "lsof +L1"],
        related: &["disk.inodes", "disk.util", "storage.user"],
    },
    MetricDefinition {
        id: "disk.util",
        title: "Disk Utilization (busy)",
        unit: Unit::Percent,
        meaning: "區塊裝置有 I/O 在進行中的時間比例。",
        sources: &["/proc/diskstats (第 13 欄 io_ticks，單位毫秒)"],
        formula: "(io_ticks 差值 ÷ 經過毫秒數) × 100",
        pitfalls: &[
            "對 SSD/NVMe 這個數字會騙人：它們能同時處理數十個請求，100% busy 不代表已達極限。",
            "對機械硬碟則相當可靠，接近 100% 就是真的塞車了。",
            "要判斷 NVMe 是否飽和，看佇列深度與延遲（await）比看 busy% 準。",
        ],
        commands: &["iostat -xz 1（%util 欄）", "iostat -xz 1（aqu-sz、await 欄）"],
        related: &["disk.throughput", "disk.iops", "cpu.iowait"],
    },
    MetricDefinition {
        id: "gpu.clock",
        title: "GPU Clock",
        unit: Unit::Megahertz,
        meaning: "GPU 核心（SM）與顯示記憶體的目前時脈。",
        sources: &["NVML: nvmlDeviceGetClockInfo", "amdgpu / i915 sysfs"],
        formula: "直接讀取",
        pitfalls: &[
            "閒置時掉到最低時脈是正常省電行為（NVIDIA 的 P8 狀態）。",
            "達不到標稱 boost 時脈通常是功耗或溫度上限造成，不是故障。",
        ],
        commands: &["nvidia-smi -q -d CLOCK", "nvidia-smi dmon"],
        related: &["gpu.power", "gpu.temp", "gpu.pstate"],
    },
    MetricDefinition {
        id: "gpu.fan",
        title: "GPU Fan Speed",
        unit: Unit::Percent,
        meaning: "顯示卡風扇轉速。NVIDIA 回報的是最高轉速的百分比，AMD 回報的是每分鐘轉數。",
        sources: &["NVML nvmlDeviceGetFanSpeed（NVIDIA，百分比）", "hwmon 的 fan1_input（AMD，RPM）"],
        formula: "驅動直接回報，不做換算",
        pitfalls: &[
            "0% 不一定是壞掉。現代顯卡閒置時會完全停轉（zero-RPM / fan stop）。",
            "百分比是相對於這張卡的最高轉速，不同卡之間不能直接比較。",
            "NVIDIA 與 AMD 的單位不同（% vs RPM），兩者的數字不能互相比較。",
            "被動散熱卡與筆電內顯沒有自己的風扇，這一項會是 unavailable。",
            "多風扇的卡透過 NVML 只讀得到第一顆風扇。",
        ],
        commands: &["nvidia-smi -q -d TEMPERATURE", "sensors"],
        related: &["gpu.temp", "gpu.power", "gpu.pstate"],
    },
    MetricDefinition {
        id: "gpu.intel_util",
        title: "Intel GPU Utilization (estimated)",
        unit: Unit::Percent,
        meaning: "Intel 內顯的忙碌比例。這是估計值，不是驅動直接提供的數字。",
        sources: &["/sys/class/drm/card*/power/rc6_residency_ms"],
        formula: "100 − (RC6 殘留時間差 ÷ 經過時間) × 100",
        pitfalls: &[
            "i915 沒有像 amdgpu 的 gpu_busy_percent 介面，只能用 RC6（省電休眠）殘留時間反推。",
            "RC6 只反映 GPU 有沒有進入休眠，不區分是在算圖還是只是被喚醒著。",
            "要精確數字請用 intel_gpu_top，但它需要 root 或 CAP_PERFMON。",
            "sysview 會把這個值標成 estimated，不會假裝它是精確值。",
        ],
        commands: &["sudo intel_gpu_top", "cat /sys/class/drm/card0/power/rc6_residency_ms"],
        related: &["gpu.util", "gpu.clock"],
    },
    MetricDefinition {
        id: "gpu.pcie",
        title: "PCIe Link",
        unit: Unit::Count,
        meaning: "GPU 目前協商出來的 PCIe 世代與通道數。",
        sources: &["NVML: nvmlDeviceGetCurrPcieLinkGeneration / Width"],
        formula: "直接讀取",
        pitfalls: &[
            "閒置時會降到 gen1 省電，這是正常的，有負載才會拉回 gen4/5。",
            "通道數比預期少（x8 而不是 x16）通常是主機板插槽或分流配置造成。",
        ],
        commands: &["nvidia-smi -q -d PIDS,CLOCK", "lspci -vv -s <bus>"],
        related: &["gpu.util", "gpu.vram"],
    },
    MetricDefinition {
        id: "gpu.power",
        title: "GPU Power Draw",
        unit: Unit::Watts,
        meaning: "GPU 目前的耗電量與功耗上限。",
        sources: &["NVML: nvmlDeviceGetPowerUsage / GetPowerManagementLimit"],
        formula: "毫瓦 ÷ 1000",
        pitfalls: &[
            "貼著功耗上限跑代表你是被功耗限制（power limited），不是算力不足。",
            "閒置功耗因卡而異，桌機卡 10–30W 都算正常。",
            "部分卡（尤其筆電與虛擬化環境）不支援功耗讀取，會顯示 n/a。",
        ],
        commands: &["nvidia-smi -q -d POWER", "nvidia-smi dmon -s p"],
        related: &["gpu.clock", "gpu.temp", "gpu.util"],
    },
    MetricDefinition {
        id: "gpu.process",
        title: "GPU Compute Processes",
        unit: Unit::Count,
        meaning: "目前在這張 GPU 上有 CUDA/compute context 的行程，以及各自佔用的 VRAM。",
        sources: &["NVML: nvmlDeviceGetComputeRunningProcesses"],
        formula: "驅動直接列舉",
        pitfalls: &[
            "一般使用者只看得到自己的行程；要看全部使用者需要管理員權限。",
            "圖形（graphics）context 與 compute context 是分開列的。",
            "在容器裡跑的行程，PID 是宿主機 namespace 的，對不上容器內的 PID。",
        ],
        commands: &["nvidia-smi", "fuser -v /dev/nvidia*"],
        related: &["gpu.vram", "gpu.util", "proc.cpu"],
    },
    MetricDefinition {
        id: "gpu.pstate",
        title: "GPU Performance State",
        unit: Unit::Count,
        meaning: "NVIDIA 的效能狀態，P0 是全速，P8/P12 是深度省電。",
        sources: &["NVML: nvmlDeviceGetPerformanceState"],
        formula: "驅動直接回報",
        pitfalls: &[
            "閒置時停在 P8 是正常的，不是卡壞了。",
            "有負載卻上不了 P0，通常是功耗或溫度上限。",
        ],
        commands: &["nvidia-smi -q -d PERFORMANCE"],
        related: &["gpu.clock", "gpu.power"],
    },
    MetricDefinition {
        id: "gpu.temp",
        title: "GPU Temperature",
        unit: Unit::Celsius,
        meaning: "GPU 核心溫度。",
        sources: &["NVML: nvmlDeviceGetTemperature", "amdgpu hwmon"],
        formula: "直接讀取",
        pitfalls: &[
            "現代 NVIDIA 卡約 83–88°C 才開始降頻，70 幾度完全正常。",
            "記憶體（尤其 GDDR6X）溫度可能遠高於核心溫度，但多數卡不對外提供。",
        ],
        commands: &["nvidia-smi -q -d TEMPERATURE", "sensors"],
        related: &["gpu.power", "gpu.clock"],
    },
    MetricDefinition {
        id: "gpu.util",
        title: "GPU Utilization",
        unit: Unit::Percent,
        meaning: "取樣區間內，GPU 上至少有一個 kernel 在執行的時間比例。",
        sources: &["NVML: nvmlDeviceGetUtilizationRates"],
        formula: "驅動統計的「有 kernel 活動」時間 ÷ 取樣區間",
        pitfalls: &[
            "這不是 CUDA core 佔用率。跑滿 100% 不代表算力用好用滿 —— 一個只用了 1% SM 的 kernel 一直跑，也會顯示 100%。",
            "要看真正的效率，得用 Nsight Compute 看 SM occupancy 或 achieved occupancy。",
            "Tensor Core 使用率完全不在這個數字裡。",
            "訓練時如果 util 在 100% 與 0% 之間跳動，瓶頸通常在資料載入而不是 GPU。",
        ],
        commands: &["nvidia-smi dmon", "nvtop", "nsys profile", "ncu"],
        related: &["gpu.vram", "gpu.power", "gpu.clock", "gpu.process"],
    },
    MetricDefinition {
        id: "gpu.vram",
        title: "GPU Memory (VRAM)",
        unit: Unit::Bytes,
        meaning: "顯示記憶體的已用量與總量。",
        sources: &["NVML: nvmlDeviceGetMemoryInfo"],
        formula: "used / total，由驅動直接提供",
        pitfalls: &[
            "PyTorch / TensorFlow 會預先抓一大塊記憶體做快取，NVML 看到的 used 是「框架抓走的」而不是「張量實際佔用的」。",
            "要看框架內部的實際用量，用 torch.cuda.memory_allocated()。",
            "VRAM 滿了會直接 OOM，不像系統記憶體有 swap 可以撐。",
            "NVML 的 used 包含驅動與顯示輸出佔用的部分。",
        ],
        commands: &["nvidia-smi", "nvidia-smi --query-compute-apps=pid,used_memory --format=csv"],
        related: &["gpu.util", "gpu.process", "mem.used"],
    },
    MetricDefinition {
        id: "mem.available",
        title: "Memory Available",
        unit: Unit::Bytes,
        meaning: "核心估計「在不觸發 swap 的前提下」還能給新程式用的記憶體量。",
        sources: &["/proc/meminfo (MemAvailable)"],
        formula: "核心考慮了可回收的 page cache 與 slab 後估出來的值",
        pitfalls: &[
            "這才是該看的數字，不是 MemFree。MemFree 低是正常且健康的。",
            "MemAvailable 是估計值，不是保證值。",
            "Linux 3.14 以前沒有這個欄位，舊系統只能自己估。",
        ],
        commands: &["free -h", "cat /proc/meminfo"],
        related: &["mem.used", "mem.cached", "mem.swap"],
    },
    MetricDefinition {
        id: "mem.cached",
        title: "Page Cache",
        unit: Unit::Bytes,
        meaning: "核心拿空閒記憶體快取磁碟內容的量。記憶體不夠時可以立刻回收。",
        sources: &["/proc/meminfo (Cached, Buffers, SReclaimable, Shmem)"],
        formula: "Cached + SReclaimable − Shmem（Shmem 不可回收，要扣掉）",
        pitfalls: &[
            "cache 大是好事，代表記憶體沒有被浪費。這不是「記憶體被吃光」。",
            "tmpfs / shared memory 算在 Cached 裡但不能回收，所以要扣掉 Shmem。",
            "手動 drop_caches 幾乎永遠是錯的做法，只會讓之後的 I/O 變慢。",
        ],
        commands: &["free -h", "cat /proc/meminfo", "vmstat -s"],
        related: &["mem.available", "mem.used", "disk.throughput"],
    },
    MetricDefinition {
        id: "mem.swap",
        title: "Swap Usage",
        unit: Unit::Bytes,
        meaning: "被換出到磁碟的記憶體量。",
        sources: &["/proc/meminfo (SwapTotal, SwapFree)"],
        formula: "SwapTotal − SwapFree",
        pitfalls: &[
            "有 swap 被用掉不代表現在正在變慢 —— 可能是很久以前換出去、之後再也沒碰的冷頁面。",
            "真正該擔心的是 swap 進出速率（si/so），不是總量。",
            "swappiness 影響核心多積極換出，預設 60。",
        ],
        commands: &["free -h", "vmstat 1（si/so 欄）", "swapon --show"],
        related: &["mem.used", "mem.available"],
    },
    MetricDefinition {
        id: "mem.used",
        title: "Memory Used",
        unit: Unit::Bytes,
        meaning: "實際被程式佔用、無法立即回收的實體記憶體。",
        sources: &["/proc/meminfo (MemTotal, MemAvailable)"],
        formula: "MemTotal − MemAvailable",
        pitfalls: &[
            "這個定義刻意把 cache/buffers 算成「沒被用掉」，跟 free 指令的 used 欄一致。",
            "各行程 RSS 加總會遠大於這個值，因為共享函式庫被重複計算。",
            "要比較各行程真實佔用，用 PSS（smaps_rollup）而不是 RSS。",
        ],
        commands: &["free -h", "smem -tk", "cat /proc/meminfo"],
        related: &["mem.available", "mem.cached", "proc.rss", "cgroup.memory"],
    },
    MetricDefinition {
        id: "net.errors",
        title: "Network Errors / Drops",
        unit: Unit::Count,
        meaning: "介面層的錯誤封包與被丟棄封包的累計數。",
        sources: &["/proc/net/dev (errs, drop 欄)"],
        formula: "開機以來的累計值，非速率",
        pitfalls: &[
            "drop 持續增加通常是接收佇列滿了（ring buffer 太小或 CPU 來不及處理）。",
            "errs 增加常代表實體層問題：線材、光模組、雙工協商不一致。",
            "少量非零值可能是開機時的暫態，看的是有沒有持續增加。",
        ],
        commands: &["ip -s link", "ethtool -S eth0", "netstat -i"],
        related: &["net.throughput"],
    },
    MetricDefinition {
        id: "net.ip",
        title: "IP Address",
        unit: Unit::Scalar,
        meaning: "這個介面上設定的第三層（網路層）位址，含前綴長度。",
        sources: &["getifaddrs(3)"],
        formula: "直接讀取；`/24` 這種前綴長度是子網路遮罩的位元數",
        pitfalls: &[
            "一個介面可以有很多個位址。這裡列出全部，不是只有「主要」那一個。",
            "私有位址（10.x、172.16–31.x、192.168.x）在網際網路上看不到 —— 對外的位址由 NAT 決定。",
            "169.254.x.x 是 link-local，代表 DHCP 失敗了。",
            "IPv6 的 fe80:: 開頭同樣是 link-local，每個介面都會有一個，不代表有 IPv6 連線能力。",
        ],
        commands: &["ip addr show", "ip -4 addr", "ip route get 1.1.1.1"],
        related: &["net.mac", "net.throughput"],
    },
    MetricDefinition {
        id: "net.mac",
        title: "MAC Address",
        unit: Unit::Scalar,
        meaning: "網路介面的第二層（鏈路層）硬體位址。",
        sources: &["/sys/class/net/<iface>/address"],
        formula: "直接讀取",
        pitfalls: &[
            "MAC 位址是可以改的。很多系統（尤其是 Wi-Fi）預設會做隨機化以避免被追蹤。",
            "虛擬介面（bridge、veth、docker0、tun）的 MAC 是核心生成的，不對應任何實體硬體。",
            "前三個位元組是 OUI（廠商代碼），但隨機化的位址不會對應到真的廠商。",
            "MAC 只在同一個區域網段內有意義，不會跨過路由器。",
        ],
        commands: &["ip link show", "cat /sys/class/net/eth0/address"],
        related: &["net.ip", "net.throughput"],
    },
    MetricDefinition {
        id: "net.sockets",
        title: "Socket Statistics",
        unit: Unit::Count,
        meaning: "系統中各類 socket 的數量：TCP 已連線、TIME_WAIT、監聽中、UDP 等。",
        sources: &["/proc/net/sockstat", "/proc/net/tcp, tcp6, udp"],
        formula: "核心直接提供的計數",
        pitfalls: &[
            "大量 TIME_WAIT 通常是正常的（主動關閉方會停留 60 秒），不一定是問題。",
            "一般使用者只能看到 socket 屬於誰的部分資訊；完整的 socket→PID 對應需要管理員權限。",
            "orphan socket 數量接近上限會導致新連線被拒。",
        ],
        commands: &["ss -s", "ss -tanp", "netstat -s"],
        related: &["net.throughput", "proc.count"],
    },
    MetricDefinition {
        id: "net.throughput",
        title: "Network Throughput",
        unit: Unit::BytesPerSec,
        meaning: "介面每秒收送的位元組數。",
        sources: &["/proc/net/dev (第 1 欄 rx_bytes、第 9 欄 tx_bytes)"],
        formula: "(位元組累計差值) ÷ 經過秒數",
        pitfalls: &[
            "這裡是 bytes/s。一般講的 Mbps 要 ×8 ÷ 1e6。千兆網路上限約 125 MB/s。",
            "計入的是介面層位元組，含表頭；應用層有效吞吐量會略低。",
            "lo（loopback）的流量會同時算在 rx 和 tx，sysview 預設不計入總量。",
        ],
        commands: &["ip -s link", "iftop", "nload", "sar -n DEV 1"],
        related: &["net.errors", "net.sockets"],
    },
    MetricDefinition {
        id: "proc.count",
        title: "Process / Thread Count",
        unit: Unit::Count,
        meaning: "系統中的行程總數與執行緒總數。",
        sources: &["/proc/<pid>/stat (第 20 欄 num_threads)"],
        formula: "列舉 /proc 下所有數字目錄；執行緒數為各行程 num_threads 加總",
        pitfalls: &[
            "行程數包含核心執行緒（名稱被方括號包起來的那些）。",
            "接近 kernel.pid_max 或 threads-max 會導致 fork 失敗。",
        ],
        commands: &["ps -e | wc -l", "ps -eLf | wc -l", "sysctl kernel.pid_max"],
        related: &["cpu.forks", "proc.state"],
    },
    MetricDefinition {
        id: "proc.cpu",
        title: "Process CPU Usage",
        unit: Unit::Percent,
        meaning: "單一行程在取樣區間內消耗的 CPU 時間比例。",
        sources: &["/proc/<pid>/stat (第 14 utime、第 15 stime)"],
        formula: "((utime+stime) 差值 ÷ CLK_TCK ÷ 經過秒數) × 100",
        pitfalls: &[
            "可以超過 100%：一個用了 4 顆核心的多執行緒行程會顯示 400%。",
            "這是行程自己的 CPU 時間，不含已結束的子行程（那在 cutime/cstime）。",
            "行程可能在兩次取樣之間結束，這時它會直接從清單消失，不是錯誤。",
        ],
        commands: &["pidstat 1", "top -p <pid>", "perf top -p <pid>"],
        related: &["cpu.usage", "proc.state", "proc.threads"],
    },
    MetricDefinition {
        id: "proc.rss",
        title: "Process Resident Memory (RSS)",
        unit: Unit::Bytes,
        meaning: "行程目前實際佔用的實體記憶體頁面總量。",
        sources: &["/proc/<pid>/stat (第 24 欄 rss，單位為頁)"],
        formula: "rss 頁數 × 頁面大小(4096)",
        pitfalls: &[
            "共享函式庫在每個行程都被完整計算，所以把所有 RSS 加起來會遠超過實體記憶體。",
            "要公平分攤共享頁面，用 PSS（/proc/<pid>/smaps_rollup），但那要有權限且讀取較慢。",
            "RSS 不含被換出到 swap 的部分。",
        ],
        commands: &["ps -eo pid,rss,comm --sort=-rss", "smem -tk", "cat /proc/<pid>/smaps_rollup"],
        related: &["mem.used", "proc.virt", "storage.user"],
    },
    MetricDefinition {
        id: "proc.state",
        title: "Process State",
        unit: Unit::Count,
        meaning: "行程目前的排程狀態：R 執行中、S 可中斷睡眠、D 不可中斷 I/O、Z 殭屍、T 停止。",
        sources: &["/proc/<pid>/stat (第 3 欄)"],
        formula: "核心直接提供的單一字元",
        pitfalls: &[
            "D 狀態無法用 SIGKILL 殺掉。持續處於 D 通常是磁碟或 NFS 掛掉。",
            "Z（殭屍）本身不佔資源，但佔用 PID；問題出在父行程沒有 wait()。",
            "S 是絕大多數行程的常態，不代表有問題。",
        ],
        commands: &["ps -eo state,pid,user,comm", "ps -eo state,pid,comm | grep '^[DZ]'"],
        related: &["proc.cpu", "cpu.running_blocked", "cpu.iowait"],
    },
    MetricDefinition {
        id: "proc.threads",
        title: "Process Thread Count",
        unit: Unit::Count,
        meaning: "行程目前的執行緒數量。",
        sources: &["/proc/<pid>/stat (第 20 欄 num_threads)"],
        formula: "核心直接提供",
        pitfalls: &[
            "執行緒數遠超過核心數通常代表 thread pool 設定不當，context switch 會變多。",
            "JVM / Go runtime 會自己管理執行緒，數字偏高是正常的。",
        ],
        commands: &["ps -o nlwp -p <pid>", "ls /proc/<pid>/task | wc -l"],
        related: &["proc.cpu", "cpu.ctxt"],
    },
    MetricDefinition {
        id: "proc.virt",
        title: "Process Virtual Memory (VIRT)",
        unit: Unit::Bytes,
        meaning: "行程位址空間的總大小，含未實際配置實體記憶體的部分。",
        sources: &["/proc/<pid>/stat (第 23 欄 vsize)"],
        formula: "核心直接提供，單位為位元組",
        pitfalls: &[
            "VIRT 幾乎沒有參考價值。mmap 一個 1 TB 的檔案會讓 VIRT 變成 1 TB，但實體記憶體用量是 0。",
            "Go / Java runtime 會預留巨大的位址空間，VIRT 動輒數 GB 是正常的。",
            "要看真正的記憶體壓力請看 RSS 或 PSS。",
        ],
        commands: &["ps -eo pid,vsz,rss,comm", "pmap -x <pid>"],
        related: &["proc.rss", "mem.used"],
    },
    MetricDefinition {
        id: "storage.user",
        title: "Per-User Storage Usage",
        unit: Unit::Bytes,
        meaning: "各使用者家目錄實際佔用的磁碟空間，語意與 du 一致。",
        sources: &["家目錄的檔案系統遞迴走訪 (statx / lstat 的 blocks 欄)"],
        formula: "st_blocks × 512 加總；以 (device, inode) 去重處理 hard link",
        pitfalls: &[
            "用的是已配置區塊而不是檔案大小，所以稀疏檔（sparse file）不會被高估。",
            "不跟隨 symlink，也不跨檔案系統邊界，避免重複計算與無窮迴圈。",
            "需要管理員權限才能掃描其他使用者的家目錄；一般使用者只會看到自己的。",
            "這是按需掃描，不是持續取樣 —— 每秒掃 /home 會把伺服器拖垮。",
        ],
        commands: &["du -xsh /home/*", "du -x --max-depth=1 /home/user", "df -h"],
        related: &["disk.usage", "disk.inodes"],
    },
    MetricDefinition {
        id: "sys.highlights",
        title: "Highlights",
        unit: Unit::Scalar,
        meaning: "總覽頁上「現在最值得看一眼的幾件事」，由固定規則從已取樣的狀態挑出來。",
        sources: &[
            "/proc/[pid]/stat（最耗 CPU 的行程）",
            "statvfs(2)（最滿的掛載點）",
            "/proc/net/dev（最忙的介面）",
            "NVML（GPU 閒置但 VRAM 佔著）",
        ],
        formula: "各項取最大值：CPU% 最高的行程、使用率最高的掛載點、收送總和最大的實體介面",
        pitfalls: &[
            "這裡**不做推測**。每一條都是一個規則加上算出來的數字，你可以到那一頁對照。",
            "它不會告訴你「這台機器有問題」—— 只會說哪個數字最大。判斷還是你的。",
            "「最耗 CPU」是取樣區間的平均，不是瞬間值；短暫的尖峰可能不會出現在這裡。",
            "「GPU 閒置但 VRAM 佔著」代表有行程還握著顯示記憶體沒放掉，那通常是當掉的訓練腳本。",
        ],
        commands: &["ps aux --sort=-%cpu | head", "df -h", "ip -s link", "nvidia-smi"],
        related: &["proc.cpu", "disk.usage", "net.throughput", "gpu.vram"],
    },
    MetricDefinition {
        id: "sys.uptime",
        title: "System Uptime",
        unit: Unit::Seconds,
        meaning: "從開機到現在經過的時間。",
        sources: &["/proc/uptime（第一個數字是秒數）"],
        formula: "直接讀取，格式化成天/時/分",
        pitfalls: &[
            "這是「開機到現在」，不含休眠（suspend）中停掉的時間 —— 筆電合蓋期間不會累加。",
            "在容器裡讀到的是**宿主機**的開機時間，不是容器啟動時間。",
            "uptime 很長不代表系統健康；沒重開機也代表核心安全更新沒生效。",
        ],
        commands: &["uptime", "uptime -p", "cat /proc/uptime", "who -b"],
        related: &["cpu.load"],
    },
];

/// 依 id 查定義。`METRICS` 依 id 排序，所以可以二分搜尋。
pub fn lookup(id: &str) -> Option<&'static MetricDefinition> {
    METRICS
        .binary_search_by(|m| m.id.cmp(id))
        .ok()
        .map(|i| &METRICS[i])
}

/// 全部 metric id（測試與 `--list-metrics` 用）。
pub fn all_ids() -> impl Iterator<Item = &'static str> {
    METRICS.iter().map(|m| m.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_sorted_for_binary_search() {
        for w in METRICS.windows(2) {
            assert!(
                w[0].id < w[1].id,
                "METRICS 必須依 id 排序: {} 應在 {} 之前",
                w[1].id,
                w[0].id
            );
        }
    }

    #[test]
    fn every_metric_is_findable() {
        for m in METRICS {
            assert_eq!(lookup(m.id).map(|d| d.id), Some(m.id));
        }
        assert!(lookup("does.not.exist").is_none());
    }

    #[test]
    fn every_metric_is_fully_documented() {
        for m in METRICS {
            assert!(!m.title.is_empty(), "{} 缺 title", m.id);
            assert!(m.meaning.len() > 10, "{} 的 meaning 太短", m.id);
            assert!(!m.sources.is_empty(), "{} 沒寫資料來源", m.id);
            assert!(!m.formula.is_empty(), "{} 沒寫算式", m.id);
            assert!(
                !m.pitfalls.is_empty(),
                "{} 沒寫 pitfalls（這是最有價值的一欄）",
                m.id
            );
            assert!(!m.commands.is_empty(), "{} 沒給對應的原生指令", m.id);
        }
    }

    #[test]
    fn related_metrics_all_resolve() {
        for m in METRICS {
            for r in m.related {
                assert!(
                    lookup(r).is_some(),
                    "{} 的 related 指向不存在的 metric: {r}",
                    m.id
                );
            }
        }
    }
}
