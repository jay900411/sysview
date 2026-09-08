//! Collector 層：唯一會碰 `/proc`、`/sys`、NVML 的地方。
//!
//! UI **絕不**直接讀檔案。所有取樣、解析、差分、速率換算、可用性判斷
//! 都在這裡完成，往上只交出正規化過的 [`crate::metrics::model`]。

pub mod cgroup;
pub mod cpu;
pub mod disk;
pub mod gpu;
pub mod memory;
pub mod network;
pub mod process;
pub mod storage_scan;
pub mod thermal;
pub mod util;

use std::time::{Duration, Instant};

/// 各 collector 的取樣間隔。
///
/// **不要所有資料同頻率硬掃**。掃全部 `/proc/<pid>` 是最貴的動作
/// （實測佔單次取樣約 88%），而行程清單本來就不需要跟圖表一樣快。
#[derive(Debug, Clone, Copy)]
pub struct Intervals {
    /// CPU / 記憶體圖表 —— 使用者想看到即時波形，所以跟著主更新率走。
    pub fast: Duration,
    /// GPU：NVML 呼叫是微秒等級，可以跟得很近。
    pub gpu: Duration,
    /// 行程清單：最貴，獨立節流。
    pub process: Duration,
    /// 磁碟容量：每個掛載點一次 statvfs，對掛掉的 NFS 會阻塞。
    pub mounts: Duration,
    /// 溫度感測器：物理上不會毫秒級變化。
    pub thermal: Duration,
    /// IP 位址、連線速度：幾乎不變。
    pub link: Duration,
}

impl Intervals {
    pub fn from_base(base: Duration) -> Self {
        Self {
            fast: base,
            gpu: base.max(Duration::from_millis(500)),
            // 行程掃描最快 0.75 秒一次，即使使用者把主更新率調到 0.2 秒
            process: base.max(Duration::from_millis(750)),
            mounts: Duration::from_secs(5),
            thermal: Duration::from_secs(2),
            link: Duration::from_secs(10),
        }
    }
}

impl Default for Intervals {
    fn default() -> Self {
        Self::from_base(Duration::from_secs(1))
    }
}

/// 追蹤「某件事上次做是什麼時候」，用來實作獨立取樣間隔。
#[derive(Debug, Default, Clone, Copy)]
pub struct Tick {
    last: Option<Instant>,
}

impl Tick {
    /// 到期了就回 `true` 並記下時間。第一次呼叫一定到期。
    pub fn due(&mut self, now: Instant, every: Duration) -> bool {
        match self.last {
            Some(t) if now.duration_since(t) < every => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
    /// 距離上次執行過了多久。第一次回 `None`。
    pub fn elapsed(&self, now: Instant) -> Option<Duration> {
        self.last.map(|t| now.duration_since(t))
    }
}

/// 全系統狀態。UI 只讀這個。
pub struct SystemState {
    pub cpu: cpu::CpuCollector,
    pub memory: memory::MemCollector,
    pub disk: disk::DiskCollector,
    pub network: network::NetCollector,
    pub process: process::ProcCollector,
    pub gpu: gpu::GpuCollector,
    pub cgroup: cgroup::CgroupState,
    pub intervals: Intervals,

    tick_fast: Tick,
    tick_gpu: Tick,
    tick_proc: Tick,
    /// 取樣成本統計，給 `--perf` 與效能測試用。
    pub timings: Timings,
}

/// 每個 collector 上一輪花了多久。用來證明沒有哪個 collector 失控。
#[derive(Debug, Default, Clone, Copy)]
pub struct Timings {
    pub cpu: Duration,
    pub memory: Duration,
    pub disk: Duration,
    pub network: Duration,
    pub process: Duration,
    pub gpu: Duration,
}

impl Timings {
    pub fn total(&self) -> Duration {
        self.cpu + self.memory + self.disk + self.network + self.process + self.gpu
    }
}

impl SystemState {
    pub fn new(history_len: usize, intervals: Intervals) -> Self {
        Self::with_options(history_len, intervals, true)
    }

    /// `gpu_enabled = false` 會完全跳過 NVML 的 dlopen。
    /// NVML 會讓 RSS 增加約 15 MB（那是驅動自己的緩衝區），
    /// 記憶體吃緊的部署可以用 `--no-gpu` 換掉。
    pub fn with_options(history_len: usize, intervals: Intervals, gpu_enabled: bool) -> Self {
        Self {
            cpu: cpu::CpuCollector::new(history_len),
            memory: memory::MemCollector::new(history_len),
            disk: disk::DiskCollector::new(history_len),
            network: network::NetCollector::new(history_len),
            process: process::ProcCollector::new(),
            gpu: gpu::GpuCollector::new(history_len, gpu_enabled),
            cgroup: cgroup::detect(),
            intervals,
            tick_fast: Tick::default(),
            tick_gpu: Tick::default(),
            tick_proc: Tick::default(),
            timings: Timings::default(),
        }
    }

    /// 距離下一件該做的事還有多久。主迴圈靠這個睡覺，避免空轉。
    pub fn next_due(&self, now: Instant) -> Duration {
        let remaining = |tick: &Tick, every: Duration| -> Duration {
            match tick.elapsed(now) {
                Some(e) => every.saturating_sub(e),
                None => Duration::ZERO,
            }
        };
        // 背景掃描還沒回來時早點醒來收結果 —— 不然要等到下一件事到期
        // （最多 250 ms）行程清單才會換上新的一輪。
        let pending = if self.process.is_pending() {
            Duration::from_millis(20)
        } else {
            Duration::MAX
        };
        remaining(&self.tick_fast, self.intervals.fast)
            .min(remaining(&self.tick_gpu, self.intervals.gpu))
            .min(remaining(&self.tick_proc, self.intervals.process))
            .min(pending)
    }

    /// 依各自的間隔取樣。回傳 `true` 代表這輪真的取了樣。
    pub fn sample(&mut self, now: Instant) -> bool {
        macro_rules! timed {
            ($field:ident, $body:expr) => {{
                let t = Instant::now();
                $body;
                self.timings.$field = t.elapsed();
            }};
        }

        let mut any = false;
        if self.tick_fast.due(now, self.intervals.fast) {
            timed!(cpu, self.cpu.update(now));
            timed!(memory, self.memory.update());
            timed!(disk, self.disk.update(now));
            timed!(network, self.network.update(now));
            any = true;
        }
        if self.tick_gpu.due(now, self.intervals.gpu) {
            timed!(gpu, self.gpu.update(now));
            any = true;
        }
        // 行程掃描在背景執行緒上：這裡只收結果、下請求，主執行緒不等。
        // 掃描的成本由 worker 自己量，收到結果時才記進 timings。
        if self.process.poll() {
            self.timings.process = self.process.last_cost();
            self.link_gpu_processes();
            any = true;
        }
        if self.tick_proc.due(now, self.intervals.process) {
            let ncpu = self.cpu.state().logical.max(1);
            self.process.request(now, ncpu);
        }
        any
    }

    /// 一次取樣所有東西（快照模式用，不做節流）。
    pub fn sample_all(&mut self, now: Instant) {
        self.cpu.update(now);
        self.memory.update();
        self.disk.update(now);
        self.network.update(now);
        self.gpu.update(now);
        let ncpu = self.cpu.state().logical.max(1);
        self.process.update(now, ncpu);
        self.link_gpu_processes();
    }

    /// NVML 只給 GPU 行程的 PID，名稱與擁有者要從 process collector 補。
    /// 查不到通常代表那是**別的使用者**的行程 —— 這是正確的權限行為。
    fn link_gpu_processes(&mut self) {
        let table: std::collections::HashMap<u32, (String, String)> = self
            .process
            .state()
            .processes
            .iter()
            .map(|p| (p.pid as u32, (p.name.clone(), p.user.clone())))
            .collect();
        self.gpu.annotate_processes(|pid| table.get(&pid).cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_scan_is_throttled_below_main_rate() {
        // 使用者把更新率拉到 0.2 秒，行程掃描仍應維持 0.75 秒
        let i = Intervals::from_base(Duration::from_millis(200));
        assert_eq!(i.fast, Duration::from_millis(200));
        assert_eq!(
            i.process,
            Duration::from_millis(750),
            "行程掃描不可跟著飆到 5 Hz"
        );
    }

    #[test]
    fn slow_base_rate_slows_everything_proportionally() {
        let i = Intervals::from_base(Duration::from_secs(5));
        assert_eq!(
            i.process,
            Duration::from_secs(5),
            "主更新率更慢時行程掃描不該反而變快"
        );
        assert_eq!(i.gpu, Duration::from_secs(5));
    }

    #[test]
    fn expensive_collectors_have_fixed_floors() {
        let i = Intervals::from_base(Duration::from_millis(200));
        assert_eq!(
            i.mounts,
            Duration::from_secs(5),
            "statvfs 對掛掉的 NFS 會阻塞"
        );
        assert_eq!(i.link, Duration::from_secs(10));
    }

    #[test]
    fn tick_fires_first_time_then_respects_interval() {
        let mut t = Tick::default();
        let t0 = Instant::now();
        assert!(t.due(t0, Duration::from_secs(1)), "第一次一定要執行");
        assert!(!t.due(t0 + Duration::from_millis(500), Duration::from_secs(1)));
        assert!(t.due(t0 + Duration::from_millis(1001), Duration::from_secs(1)));
    }

    #[test]
    fn tick_elapsed_is_none_before_first_fire() {
        let mut t = Tick::default();
        assert!(t.elapsed(Instant::now()).is_none());
        let now = Instant::now();
        t.due(now, Duration::from_secs(1));
        assert!(t.elapsed(now + Duration::from_secs(2)).is_some());
    }

    #[test]
    fn timings_sum_all_collectors() {
        let t = Timings {
            cpu: Duration::from_millis(1),
            memory: Duration::from_millis(2),
            disk: Duration::from_millis(3),
            network: Duration::from_millis(4),
            process: Duration::from_millis(5),
            gpu: Duration::from_millis(6),
        };
        assert_eq!(t.total(), Duration::from_millis(21));
    }
}
