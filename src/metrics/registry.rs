//! 把 UI 上「目前選到的東西」對應到 metric id，讓 `e` 鍵知道要解釋什麼。

use crate::metrics::knowledge::{self, MetricDefinition};

/// UI 中可被選取、可解釋的一列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focusable {
    /// 顯示名稱
    pub label: &'static str,
    /// 對應的 metric id
    pub metric_id: &'static str,
}

impl Focusable {
    pub const fn new(label: &'static str, metric_id: &'static str) -> Self {
        Self { label, metric_id }
    }
    pub fn definition(&self) -> Option<&'static MetricDefinition> {
        knowledge::lookup(self.metric_id)
    }
}

/// 每個頁面可解釋的 metric 清單。按 `e` 時，UI 會用目前選取的索引查這張表。
pub struct PageMetrics;

impl PageMetrics {
    pub const CPU: &'static [Focusable] = &[
        Focusable::new("CPU Utilization", "cpu.usage"),
        Focusable::new("Model", "cpu.model"),
        Focusable::new("Logical CPUs", "cpu.logical"),
        Focusable::new("Load Average", "cpu.load"),
        Focusable::new("I/O Wait", "cpu.iowait"),
        Focusable::new("Steal Time", "cpu.steal"),
        Focusable::new("Frequency", "cpu.freq"),
        Focusable::new("Temperature", "cpu.temp"),
        Focusable::new("Context Switches", "cpu.ctxt"),
        Focusable::new("Interrupts", "cpu.intr"),
        Focusable::new("Load per Core", "cpu.load_per_core"),
        Focusable::new("Uptime", "sys.uptime"),
        Focusable::new("Highlights", "sys.highlights"),
        Focusable::new("Process Creation", "cpu.forks"),
        Focusable::new("Running / Blocked", "cpu.running_blocked"),
        Focusable::new("cgroup CPU Limit", "cgroup.cpu"),
    ];
    pub const MEMORY: &'static [Focusable] = &[
        Focusable::new("Memory Used", "mem.used"),
        Focusable::new("Memory Available", "mem.available"),
        Focusable::new("Page Cache", "mem.cached"),
        Focusable::new("Swap", "mem.swap"),
        Focusable::new("Process RSS", "proc.rss"),
        Focusable::new("cgroup Memory Limit", "cgroup.memory"),
    ];
    pub const GPU: &'static [Focusable] = &[
        Focusable::new("GPU Utilization", "gpu.util"),
        Focusable::new("VRAM", "gpu.vram"),
        Focusable::new("Temperature", "gpu.temp"),
        Focusable::new("Power", "gpu.power"),
        Focusable::new("Clocks", "gpu.clock"),
        Focusable::new("Performance State", "gpu.pstate"),
        Focusable::new("PCIe Link", "gpu.pcie"),
        Focusable::new("Compute Processes", "gpu.process"),
        Focusable::new("Fan Speed", "gpu.fan"),
        Focusable::new("Intel iGPU (estimated)", "gpu.intel_util"),
    ];
    pub const STORAGE: &'static [Focusable] = &[
        Focusable::new("Filesystem Usage", "disk.usage"),
        Focusable::new("Inode Usage", "disk.inodes"),
        Focusable::new("Throughput", "disk.throughput"),
        Focusable::new("IOPS", "disk.iops"),
        Focusable::new("Device Busy", "disk.util"),
        Focusable::new("Per-User Storage", "storage.user"),
    ];
    pub const NETWORK: &'static [Focusable] = &[
        Focusable::new("Throughput", "net.throughput"),
        Focusable::new("Errors / Drops", "net.errors"),
        Focusable::new("Sockets", "net.sockets"),
        Focusable::new("MAC Address", "net.mac"),
        Focusable::new("IP Address", "net.ip"),
    ];
    pub const PROCESS: &'static [Focusable] = &[
        Focusable::new("Process CPU", "proc.cpu"),
        Focusable::new("Resident Memory", "proc.rss"),
        Focusable::new("Virtual Memory", "proc.virt"),
        Focusable::new("Threads", "proc.threads"),
        Focusable::new("State", "proc.state"),
        Focusable::new("Process Count", "proc.count"),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_focusable_resolves_to_a_definition() {
        let pages: [(&str, &[Focusable]); 6] = [
            ("cpu", PageMetrics::CPU),
            ("memory", PageMetrics::MEMORY),
            ("gpu", PageMetrics::GPU),
            ("storage", PageMetrics::STORAGE),
            ("network", PageMetrics::NETWORK),
            ("process", PageMetrics::PROCESS),
        ];
        for (page, list) in pages {
            assert!(!list.is_empty(), "{page} 頁沒有任何可解釋的 metric");
            for f in list {
                assert!(
                    f.definition().is_some(),
                    "{page} 頁的 {:?} 指向不存在的 metric id {}",
                    f.label,
                    f.metric_id
                );
            }
        }
    }

    #[test]
    fn all_knowledge_entries_are_reachable_from_some_page() {
        let all: Vec<&Focusable> = [
            PageMetrics::CPU,
            PageMetrics::MEMORY,
            PageMetrics::GPU,
            PageMetrics::STORAGE,
            PageMetrics::NETWORK,
            PageMetrics::PROCESS,
        ]
        .concat()
        .leak()
        .iter()
        .collect();
        let reachable: std::collections::HashSet<&str> = all.iter().map(|f| f.metric_id).collect();
        let unreachable: Vec<&str> = knowledge::all_ids()
            .filter(|id| !reachable.contains(id))
            .collect();
        assert!(
            unreachable.is_empty(),
            "這些 metric 有寫說明但使用者按不到，等於白寫: {unreachable:?}"
        );
    }
}
