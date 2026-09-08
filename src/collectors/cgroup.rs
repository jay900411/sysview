//! cgroup v2 感知。
//!
//! 在容器裡，`/proc/stat` 與 `/proc/meminfo` 顯示的是**宿主機**的數字。
//! 直接把宿主機的 64 GB 當成自己的可用記憶體，是容器監控最常見的錯誤。
//! 這個模組偵測我們是否被 cgroup 限制，並讀出真正的配額。

use crate::collectors::util;
use crate::metrics::model::{Reading, Unit};

/// 執行環境。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    /// 直接跑在實體/虛擬機上，沒有容器化限制。
    Host,
    /// 在容器裡（偵測到 /.dockerenv、容器 runtime 或非 root cgroup 路徑）。
    Container,
    /// 在 cgroup 裡但看起來不像容器（例如 systemd 的 user slice）。
    Cgroup,
}

impl Environment {
    pub fn label(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Container => "container",
            Self::Cgroup => "cgroup",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CgroupState {
    pub environment: Environment,
    /// cgroup v2 的相對路徑，例如 `/user.slice/user-1000.slice`
    pub path: Option<String>,
    /// CPU 配額換算成等效核心數。`None` = 無上限。
    pub cpu_limit_cores: Option<f64>,
    pub cpu_throttled_periods: Option<u64>,
    /// 記憶體上限。`None` = 無上限（memory.max 是 "max"）。
    pub memory_limit: Option<u64>,
    pub memory_current: Option<u64>,
    pub v2: bool,
}

impl Default for CgroupState {
    fn default() -> Self {
        Self {
            environment: Environment::Host,
            path: None,
            cpu_limit_cores: None,
            cpu_throttled_periods: None,
            memory_limit: None,
            memory_current: None,
            v2: false,
        }
    }
}

impl CgroupState {
    /// 記憶體使用率（相對於 cgroup 配額，不是宿主機總量）。
    pub fn memory_percent(&self) -> Option<Reading> {
        let (cur, max) = (self.memory_current?, self.memory_limit?);
        if max == 0 {
            return None;
        }
        Some(
            Reading::exact(cur as f64 / max as f64 * 100.0, Unit::Percent).with_id("cgroup.memory"),
        )
    }
    /// 有沒有任何實際限制。沒有限制就不必在 UI 上佔版面。
    pub fn is_limited(&self) -> bool {
        self.cpu_limit_cores.is_some() || self.memory_limit.is_some()
    }
}

/// 解析 cgroup v2 的 `cpu.max`：格式是 `<quota> <period>` 或 `max <period>`。
pub fn parse_cpu_max(text: &str) -> Option<f64> {
    let mut it = text.split_ascii_whitespace();
    let quota = it.next()?;
    if quota == "max" {
        return None; // 無上限
    }
    let quota: f64 = quota.parse().ok()?;
    let period: f64 = it.next()?.parse().ok()?;
    if period <= 0.0 {
        return None;
    }
    Some(quota / period)
}

/// 解析 `memory.max`：數字或字串 `max`。
pub fn parse_memory_max(text: &str) -> Option<u64> {
    let t = text.trim();
    if t == "max" {
        return None;
    }
    t.parse().ok()
}

/// 從 `/proc/self/cgroup` 取出 v2 的相對路徑。
/// v2 統一格式是 `0::/path`。
pub fn parse_self_cgroup(text: &str) -> Option<String> {
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let hier = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        if hier == "0" && controllers.is_empty() {
            return Some(path.to_owned());
        }
    }
    None
}

pub fn detect() -> CgroupState {
    let mut s = CgroupState::default();
    let root = "/sys/fs/cgroup";
    s.v2 = std::path::Path::new(&format!("{root}/cgroup.controllers")).exists();
    s.path = util::read_string("/proc/self/cgroup")
        .ok()
        .as_deref()
        .and_then(parse_self_cgroup);

    if s.v2 {
        s.cpu_limit_cores = util::read_string(format!("{root}/cpu.max"))
            .ok()
            .as_deref()
            .and_then(parse_cpu_max);
        s.memory_limit = util::read_string(format!("{root}/memory.max"))
            .ok()
            .as_deref()
            .and_then(parse_memory_max);
        s.memory_current = util::read_i64_opt(format!("{root}/memory.current")).map(|v| v as u64);
        if let Ok(stat) = util::read_string(format!("{root}/cpu.stat")) {
            s.cpu_throttled_periods = stat
                .lines()
                .find_map(|l| l.strip_prefix("nr_throttled ")?.trim().parse().ok());
        }
    }

    s.environment = detect_environment(s.path.as_deref());
    s
}

fn detect_environment(cgroup_path: Option<&str>) -> Environment {
    if std::path::Path::new("/.dockerenv").exists()
        || std::path::Path::new("/run/.containerenv").exists()
    {
        return Environment::Container;
    }
    match cgroup_path {
        Some(p)
            if p.contains("/docker/")
                || p.contains("/kubepods")
                || p.contains("/containerd")
                || p.contains("/lxc/")
                || p.contains(".scope/") && p.contains("docker") =>
        {
            Environment::Container
        }
        Some(p) if p != "/" => Environment::Cgroup,
        _ => Environment::Host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpu_quota_as_core_count() {
        assert_eq!(parse_cpu_max("200000 100000"), Some(2.0));
        assert_eq!(parse_cpu_max("50000 100000"), Some(0.5));
    }

    #[test]
    fn unlimited_cpu_is_none_not_zero() {
        assert_eq!(
            parse_cpu_max("max 100000"),
            None,
            "\"max\" 代表無上限，不是 0"
        );
    }

    #[test]
    fn rejects_bad_cpu_max() {
        assert_eq!(parse_cpu_max(""), None);
        assert_eq!(parse_cpu_max("100000"), None, "缺 period 欄位");
        assert_eq!(parse_cpu_max("100000 0"), None, "period 為 0 不可除以零");
    }

    #[test]
    fn parses_memory_max() {
        assert_eq!(parse_memory_max("2147483648\n"), Some(2147483648));
        assert_eq!(parse_memory_max("max\n"), None);
        assert_eq!(parse_memory_max("garbage"), None);
    }

    #[test]
    fn parses_v2_self_cgroup() {
        assert_eq!(
            parse_self_cgroup("0::/user.slice/user-1000.slice/session-3.scope\n"),
            Some("/user.slice/user-1000.slice/session-3.scope".into())
        );
    }

    #[test]
    fn ignores_v1_controller_lines() {
        let v1 = "12:pids:/user.slice\n11:memory:/user.slice\n0::/target\n";
        assert_eq!(parse_self_cgroup(v1), Some("/target".into()));
        assert_eq!(
            parse_self_cgroup("12:pids:/user.slice\n"),
            None,
            "純 v1 沒有 0:: 行"
        );
    }

    #[test]
    fn detects_container_from_cgroup_path() {
        assert_eq!(
            detect_environment(Some("/docker/abc123")),
            Environment::Container
        );
        assert_eq!(
            detect_environment(Some("/kubepods/besteffort/pod123")),
            Environment::Container
        );
        assert_eq!(detect_environment(Some("/user.slice")), Environment::Cgroup);
        assert_eq!(detect_environment(Some("/")), Environment::Host);
        assert_eq!(detect_environment(None), Environment::Host);
    }

    #[test]
    fn memory_percent_is_relative_to_quota_not_host() {
        let s = CgroupState {
            memory_limit: Some(1000),
            memory_current: Some(250),
            ..Default::default()
        };
        assert_eq!(s.memory_percent().and_then(|r| r.get()), Some(25.0));
        assert!(s.is_limited());
    }

    #[test]
    fn unlimited_cgroup_reports_no_percentage() {
        let s = CgroupState {
            memory_current: Some(250),
            ..Default::default()
        };
        assert!(s.memory_percent().is_none(), "沒有上限就沒有百分比可言");
        assert!(!s.is_limited());
    }
}
