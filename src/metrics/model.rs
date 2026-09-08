//! 正規化後的 metric 模型：collector 與 UI 之間唯一的介面。
//!
//! UI 只讀這一層，不會自己去 `read_to_string("/proc/...")`。
//! 每個數值都帶著「單位」「來源」「這是精確值還是估計值」，
//! 因為 sysview 的賣點之一就是使用者能知道數字從哪來、可不可信。

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

use crate::error::Unavailable;

/// 數值的單位。決定 UI 怎麼格式化，也決定 Explain 頁怎麼說明。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    /// 0–100
    Percent,
    Bytes,
    BytesPerSec,
    Celsius,
    Watts,
    Megahertz,
    Count,
    CountPerSec,
    Seconds,
    /// 無單位（例如 load average）
    Scalar,
}

impl Unit {
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Percent => "%",
            Self::Bytes => "B",
            Self::BytesPerSec => "B/s",
            Self::Celsius => "°C",
            Self::Watts => "W",
            Self::Megahertz => "MHz",
            Self::Count | Self::Scalar => "",
            Self::CountPerSec => "/s",
            Self::Seconds => "s",
        }
    }
}

/// 這個數字有多可信。**絕不編造數字**是本專案的硬性要求，
/// 拿不到就是拿不到，是估計值就要標成估計值。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    /// 直接從核心/驅動讀到的精確值。
    Exact,
    /// 由其他訊號推算出來的估計值（例如 Intel iGPU 用 RC6 殘留反推忙碌率）。
    /// `method` 會顯示在 UI 與 Explain 裡。
    Estimated { method: &'static str },
    /// 拿不到，附上原因。
    Unavailable(Unavailable),
}

impl Quality {
    pub fn is_estimated(&self) -> bool {
        matches!(self, Self::Estimated { .. })
    }
    pub fn is_available(&self) -> bool {
        !matches!(self, Self::Unavailable(_))
    }
}

/// 一個帶完整 provenance 的量測值。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reading {
    pub value: Option<f64>,
    pub unit: Unit,
    pub quality: Quality,
    /// 對應 [`crate::metrics::knowledge`] 裡的 metric id，按 `e` 可查說明。
    pub metric_id: Option<&'static str>,
}

impl Reading {
    pub fn exact(value: f64, unit: Unit) -> Self {
        Self {
            value: Some(value),
            unit,
            quality: Quality::Exact,
            metric_id: None,
        }
    }
    pub fn estimated(value: f64, unit: Unit, method: &'static str) -> Self {
        Self {
            value: Some(value),
            unit,
            quality: Quality::Estimated { method },
            metric_id: None,
        }
    }
    pub fn unavailable(unit: Unit, why: Unavailable) -> Self {
        Self {
            value: None,
            unit,
            quality: Quality::Unavailable(why),
            metric_id: None,
        }
    }
    pub fn unsupported(unit: Unit) -> Self {
        Self::unavailable(unit, Unavailable::Unsupported)
    }
    /// 從 `Option` 建立：`None` 視為不支援。
    pub fn from_opt(v: Option<f64>, unit: Unit) -> Self {
        match v {
            Some(v) => Self::exact(v, unit),
            None => Self::unsupported(unit),
        }
    }
    pub fn with_id(mut self, id: &'static str) -> Self {
        self.metric_id = Some(id);
        self
    }
    pub fn get(&self) -> Option<f64> {
        self.value
    }
    /// 給 gauge 用：拿不到就當 0，但呼叫端仍應用 `quality` 決定要不要標 n/a。
    pub fn or_zero(&self) -> f64 {
        self.value.unwrap_or(0.0)
    }
}

impl Default for Reading {
    /// 預設是「還沒取樣」而不是 0 —— 絕不讓未知看起來像已知。
    fn default() -> Self {
        Self::unavailable(Unit::Scalar, Unavailable::Pending)
    }
}

/// 固定長度的時間序列。歷史長度有上限，避免長時間執行時記憶體無限成長
/// （Availability 要求：不得有無上限的 allocation）。
#[derive(Clone, Debug)]
pub struct Series {
    buf: VecDeque<f64>,
    cap: usize,
}

impl Series {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap.min(4096)),
            cap: cap.max(2),
        }
    }
    pub fn push(&mut self, v: f64) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(if v.is_finite() { v } else { 0.0 });
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    pub fn last(&self) -> Option<f64> {
        self.buf.back().copied()
    }
    pub fn max(&self) -> f64 {
        self.buf.iter().copied().fold(0.0_f64, f64::max)
    }
    /// 取最後 `n` 個點（畫圖用）。
    pub fn tail(&self, n: usize) -> impl Iterator<Item = f64> + '_ {
        let skip = self.buf.len().saturating_sub(n);
        self.buf.iter().skip(skip).copied()
    }
    pub fn iter(&self) -> impl Iterator<Item = f64> + '_ {
        self.buf.iter().copied()
    }
    pub fn capacity(&self) -> usize {
        self.cap
    }
}

impl Default for Series {
    fn default() -> Self {
        Self::new(240)
    }
}

/// 整體嚴重程度。**不能只靠顏色傳達**，所以同時帶一個符號給色盲使用者。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Ok,
    Notice,
    Warning,
    Critical,
    Unknown,
}

impl Severity {
    /// 依百分比分級（通用門檻）。
    pub fn from_percent(p: f64) -> Self {
        if !p.is_finite() {
            Self::Unknown
        } else if p >= 90.0 {
            Self::Critical
        } else if p >= 75.0 {
            Self::Warning
        } else if p >= 50.0 {
            Self::Notice
        } else {
            Self::Ok
        }
    }
    /// 給色盲使用者的文字/符號提示，永遠與顏色並存。
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Ok => "●",
            Self::Notice => "◐",
            Self::Warning => "▲",
            Self::Critical => "■",
            Self::Unknown => "○",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Notice => "notice",
            Self::Warning => "WARN",
            Self::Critical => "CRIT",
            Self::Unknown => "n/a",
        }
    }
}
