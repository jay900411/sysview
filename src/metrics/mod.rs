//! Metric 層：collector 與 UI 之間的正規化模型，以及 Explain 用的知識庫。

pub mod describe;
pub mod knowledge;
pub mod model;
pub mod registry;

pub use knowledge::{lookup, MetricDefinition};
pub use model::{Quality, Reading, Series, Severity, Unit};
