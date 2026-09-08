//! Describe 系統：把畫面上任何一個可選的東西，變成一份可驗證的說明。
//!
//! # 為什麼要分兩類
//!
//! sysview 顯示的資訊有兩種本質不同的東西：
//!
//! * **固定 metric** —— `cpu.iowait`、`mem.available`、`disk.util`。
//!   欄位名稱與意義在編譯期就確定，可以事先寫死完整定義。
//!
//! * **動態 entity** —— `ens192`、`NVIDIA GeForce RTX 4070 Ti`、`PID 18423`、
//!   `/home/alice/datasets`。它們的**名字**每台機器都不一樣，不可能預先寫死。
//!
//! 對 entity，我們固定解釋「這**種**東西是什麼」，再把這台機器上的實際
//! metadata 填進去。名字是 runtime 值，型別知識才是我們寫死的部分。
//!
//! # 硬性規則
//!
//! * **不用 LLM。** 全部是 deterministic 的 Linux 知識 + 執行期狀態。
//! * **不猜語意。** `/home/jay/project` 只會說「這是一個目錄，擁有者是 jay」，
//!   不會說「這是開發專案」。作業系統沒告訴我們的事，就不說。
//! * **重用 collector 已經有的狀態**，不為了顯示說明而去 fork 一堆指令。
//!   `Equivalent commands` 是給使用者自己查用的，不代表 sysview 執行過它。
//! * **不繞過權限。** 需要管理員才看得到的東西就標示出來，不偷偷提權讀取。

use crate::collectors::SystemState;
use crate::metrics::knowledge::{self, MetricDefinition};

/// 使用者按 `e` 時，要解釋的對象。
#[derive(Clone, Debug, PartialEq)]
pub enum DescribeTarget {
    /// 固定欄位，見 [`crate::metrics::knowledge`]。
    Metric(&'static str),
    /// 這台機器上的某個具體東西。
    Entity(EntityRef),
}

impl DescribeTarget {
    pub fn metric(id: &'static str) -> Self {
        Self::Metric(id)
    }
}

/// 指向這台機器上一個具體的系統物件。
///
/// 帶的是**識別資訊**而不是快照 —— Describe 時才去目前的狀態裡查，
/// 這樣顯示的永遠是當下的值，而不是登記當時的舊值。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EntityRef {
    /// `(pid, starttime)` 一起用才能唯一識別 —— PID 會被重用。
    Process {
        pid: i32,
        starttime: u64,
    },
    NetworkInterface(String),
    /// GPU 的內部 id：NVIDIA 是 `nv0`，DRM 是 `card1`。
    Gpu(String),
    /// 掛載點路徑
    Mount(String),
    /// 區塊裝置名稱，例如 `nvme0n1`
    Disk(String),
    User(u32),
    /// 檔案系統路徑（管理員的儲存明細會用到）
    Path(String),
    /// 單一邏輯核心
    CpuCore(usize),
}

impl EntityRef {
    /// 這種東西的型別名稱（顯示在 Describe 的 Type 欄）。
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Process { .. } => "Linux Process",
            Self::NetworkInterface(_) => "Network Interface",
            Self::Gpu(_) => "GPU Device",
            Self::Mount(_) => "Mounted Filesystem",
            Self::Disk(_) => "Block Device",
            Self::User(_) => "System User",
            Self::Path(_) => "Filesystem Path",
            Self::CpuCore(_) => "Logical CPU",
        }
    }
}

/// 一個值有多確定。**絕不把推測講成事實。**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Certainty {
    /// 直接從核心/驅動讀到的
    Exact,
    /// 由其他精確值算出來的
    Derived,
    /// 由間接訊號推估的（例如 Intel iGPU 的 RC6 反推）
    Estimated,
    /// 拿不到
    Unavailable,
}

impl Certainty {
    pub fn tag(self) -> Option<&'static str> {
        match self {
            Self::Exact => None,
            Self::Derived => Some("derived"),
            Self::Estimated => Some("estimated"),
            Self::Unavailable => Some("unavailable"),
        }
    }
}

/// Describe 面板裡的一列。
#[derive(Clone, Debug)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub certainty: Certainty,
}

impl Field {
    pub fn exact(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            certainty: Certainty::Exact,
        }
    }
    pub fn derived(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            certainty: Certainty::Derived,
        }
    }
    pub fn estimated(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            certainty: Certainty::Estimated,
        }
    }
    pub fn unavailable(label: impl Into<String>, why: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: why.into(),
            certainty: Certainty::Unavailable,
        }
    }
}

/// Describe 的統一輸出。UI 只負責把它畫出來。
///
/// 不適用的區塊留空，UI 就不會顯示 —— 不要出現 `Formula: n/a` 這種噪音。
#[derive(Clone, Debug, Default)]
pub struct DescribeContent {
    pub title: String,
    pub type_name: &'static str,
    /// 固定 metric 的 id，讓使用者知道可以用 `sysview --explain <id>` 再查一次。
    /// entity 沒有 id。
    pub metric_id: Option<&'static str>,
    /// What is this? —— 這**種**東西是什麼
    pub summary: String,
    /// Current —— 這台機器上的它現在是什麼
    pub current: Vec<Field>,
    /// Source —— 資料真正的來源（要跟 collector 實作一致）
    pub source: Vec<String>,
    /// How sysview got it —— 取得方式，跟上面的來源互補
    pub how_obtained: Option<String>,
    /// Formula / Derivation
    pub derivation: Option<String>,
    /// Things to know
    pub pitfalls: Vec<String>,
    /// Equivalent commands —— 給使用者自己查，sysview 不一定執行過
    pub commands: Vec<String>,
    pub related: Vec<String>,
    /// 需要管理員權限時的說明
    pub permission_note: Option<String>,
}

impl DescribeContent {
    fn new(title: impl Into<String>, type_name: &'static str) -> Self {
        Self {
            title: title.into(),
            type_name,
            ..Default::default()
        }
    }
    /// 這份說明是不是完全空的（測試用：不該存在無法說明的可選項）。
    pub fn is_empty(&self) -> bool {
        self.summary.is_empty() && self.current.is_empty() && self.source.is_empty()
    }
}

/// 把一個 [`DescribeTarget`] 變成可顯示的說明。
///
/// 全部資料來自 `state`（collector 已經取好的）與編譯期的知識庫。
/// **不 fork 任何行程、不執行任何指令。**
pub fn describe(target: &DescribeTarget, state: &SystemState) -> DescribeContent {
    match target {
        DescribeTarget::Metric(id) => describe_metric(id, state),
        DescribeTarget::Entity(e) => entity::describe(e, state),
    }
}

/// 固定 metric 的說明：知識庫 + 這台機器上的當前值。
fn describe_metric(id: &str, state: &SystemState) -> DescribeContent {
    let Some(def) = knowledge::lookup(id) else {
        // 這不該發生。發生了就明講，不要靜默給一份空說明。
        let mut c = DescribeContent::new(id.to_owned(), "Unknown Metric");
        c.summary = format!("sysview 內部錯誤：找不到 metric「{id}」的定義。請回報。");
        return c;
    };
    from_definition(def, state)
}

fn from_definition(def: &'static MetricDefinition, state: &SystemState) -> DescribeContent {
    let mut c = DescribeContent::new(def.title, "Metric");
    c.metric_id = Some(def.id);
    c.summary = def.meaning.to_owned();
    c.source = def.sources.iter().map(|s| (*s).to_owned()).collect();
    c.derivation = Some(def.formula.to_owned());
    c.pitfalls = def.pitfalls.iter().map(|s| (*s).to_owned()).collect();
    c.commands = def.commands.iter().map(|s| (*s).to_owned()).collect();
    c.related = def.related.iter().map(|s| (*s).to_owned()).collect();
    c.current = current::for_metric(def.id, state);

    // GPU 的來源會依實際 backend 改變（NVML / i915 RC6 / amdgpu sysfs）。
    // 知識庫寫的是通則，這裡用執行期真正用到的那個覆蓋掉，
    // 否則 Explain 會對使用者說謊。
    if def.id.starts_with("gpu.") {
        if let Some(runtime) = runtime_gpu_source(state) {
            c.source = runtime;
            c.how_obtained = Some("以上是這台機器上實際使用的 backend，不是通則。".to_owned());
        }
    }
    c
}

fn runtime_gpu_source(state: &SystemState) -> Option<Vec<String>> {
    let devices = &state.gpu.state().devices;
    if devices.is_empty() {
        return None;
    }
    Some(
        devices
            .iter()
            .map(|d| format!("{} — {}", d.name, d.backend))
            .collect(),
    )
}

pub mod current;
pub mod entity;

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> SystemState {
        let mut s = SystemState::with_options(60, crate::collectors::Intervals::default(), true);
        s.sample_all(std::time::Instant::now());
        std::thread::sleep(std::time::Duration::from_millis(120));
        s.sample_all(std::time::Instant::now());
        s
    }

    #[test]
    fn every_metric_produces_a_usable_description() {
        let st = state();
        for m in knowledge::METRICS {
            let c = describe(&DescribeTarget::Metric(m.id), &st);
            assert!(!c.is_empty(), "{} 產生了空的說明", m.id);
            assert!(!c.summary.is_empty(), "{} 缺 What is this?", m.id);
            assert!(!c.source.is_empty(), "{} 缺 Source", m.id);
            assert!(!c.pitfalls.is_empty(), "{} 缺 Things to know", m.id);
            assert!(!c.commands.is_empty(), "{} 缺 Equivalent commands", m.id);
            assert!(c.derivation.is_some(), "{} 缺 Formula", m.id);
        }
    }

    #[test]
    fn unknown_metric_says_so_instead_of_going_blank() {
        let st = state();
        let c = describe(&DescribeTarget::Metric("does.not.exist"), &st);
        assert!(
            c.summary.contains("找不到"),
            "未知 metric 應明講，而不是給空說明"
        );
    }

    #[test]
    fn gpu_source_reflects_the_backend_actually_in_use() {
        let st = state();
        if st.gpu.state().devices.is_empty() {
            return;
        }
        let c = describe(&DescribeTarget::Metric("gpu.util"), &st);
        // 來源必須是執行期真的用到的 backend，不是知識庫寫的通則
        let joined = c.source.join(" ");
        let backends: Vec<&str> = st.gpu.state().devices.iter().map(|d| d.backend).collect();
        assert!(
            backends.iter().any(|b| joined.contains(b)),
            "GPU 的 Source 應反映實際 backend，實際是 {backends:?}，顯示的是 {joined}"
        );
    }

    #[test]
    fn certainty_tags_do_not_label_exact_values() {
        assert_eq!(Certainty::Exact.tag(), None, "精確值不該被加註記");
        assert_eq!(Certainty::Estimated.tag(), Some("estimated"));
        assert_eq!(Certainty::Derived.tag(), Some("derived"));
    }

    #[test]
    fn entity_type_names_are_all_distinct() {
        let refs = [
            EntityRef::Process {
                pid: 1,
                starttime: 1,
            },
            EntityRef::NetworkInterface("lo".into()),
            EntityRef::Gpu("nv0".into()),
            EntityRef::Mount("/".into()),
            EntityRef::Disk("sda".into()),
            EntityRef::User(0),
            EntityRef::Path("/tmp".into()),
            EntityRef::CpuCore(0),
        ];
        let names: std::collections::HashSet<&str> = refs.iter().map(|r| r.type_name()).collect();
        assert_eq!(names.len(), refs.len(), "每種 entity 都要有自己的型別名稱");
    }
}
