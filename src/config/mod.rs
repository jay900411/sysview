//! 使用者設定：`~/.config/sysview/config.toml`。
//!
//! # 硬性規則：設定檔**不能**影響授權
//!
//! 這裡沒有 `admin = true`、沒有 `allow_root`、沒有 `helper_path`。
//! 授權完全由 `sudo` / sudoers 決定。設定檔只管外觀與取樣頻率 ——
//! 一個能被使用者編輯的檔案，永遠不該是安全決策的依據。

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 主更新間隔（秒）。
    pub interval: f64,
    /// 主題名稱，見 [`crate::theme::THEMES`]。
    pub theme: String,
    /// 啟動時顯示哪一頁。
    pub default_page: String,
    /// 圖表保留幾個歷史取樣點。
    pub history: usize,
    /// 溫度單位：`celsius` 或 `fahrenheit`。
    pub temperature_unit: String,
    /// 圖表樣式：`area` 或 `line`。
    pub graph_style: String,
    /// 是否在總覽顯示每個核心的長條。
    pub show_per_core: bool,
    /// 行程列表預設排序：`cpu` / `rss` / `pid` / `thr` / `user`。
    pub process_sort: String,
    /// 是否啟用 GPU 監控。
    ///
    /// 關掉的話會跳過 NVML 的 dlopen，RSS 少約 15 MB（那是 NVIDIA 驅動
    /// 自己的緩衝區，不是 sysview 的）。適合記憶體吃緊、又不需要看 GPU 的機器。
    pub gpu: bool,

    /// 品牌識別：名稱、logo、吉祥物。
    pub branding: Branding,
    /// 視覺語言：pattern、圖表樣式、密度、裝飾層級。
    pub visual: Visual,
    /// 各區塊的自訂別名。原本的名字不會被取代，只是加在前面。
    pub labels: Labels,
}

/// `[branding]` —— 品牌識別。
///
/// 這一段**完全不影響授權**。名稱、吉祥物、logo 都只是畫面上的東西，
/// 不會、也不可能改變 sudo 能不能過。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Branding {
    /// 品牌代號。只收 A–Z 與 0–9，1–6 字，會自動轉大寫。
    /// 空字串代表沿用 sysview 預設品牌。
    pub name: String,
    /// logo 呈現方式：`auto` | `pixel` | `small` | `text` | `off`。
    pub logo: String,
    /// 吉祥物：`fox` | `deer` | `auto` | `none`。
    pub mascot: String,
    /// 吉祥物的選擇方式：`reactive`（跟著系統狀態）| `manual`（固定）| `idle`（只在閒置時出現）。
    pub mascot_mode: String,
    /// `manual` 時要固定在哪個狀態。
    pub mascot_state: String,
    /// 吉祥物動畫的每秒張數。上限刻意壓很低 —— 這是裝飾，不該吃 CPU。
    pub mascot_fps: u8,
    /// 啟動時顯示品牌畫面。
    ///
    /// 預設開啟。它 1.5 秒後自己消失，任何鍵也能立刻跳過 ——
    /// 一個要人動手關掉的啟動畫面是在擋路，會自己走開的就不是。
    /// `--snapshot` 與 `--json` 根本不進 TUI，完全不受影響。
    pub splash: bool,
}

impl Default for Branding {
    fn default() -> Self {
        Self {
            name: String::new(),
            logo: "auto".into(),
            mascot: "auto".into(),
            mascot_mode: "reactive".into(),
            mascot_state: "observe".into(),
            mascot_fps: 3,
            splash: true,
        }
    }
}

/// `[visual]` —— 視覺語言。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Visual {
    /// 覆蓋頂層的 `theme`（比較新的寫法，兩種都收）。
    pub theme: Option<String>,
    /// 覆蓋頂層的 `graph_style`。
    pub graph_style: Option<String>,
    /// 組成類長條的填滿樣式：`solid` | `shade` | `dot` | `braille` | `line` | `hatch` | `digital`。
    pub pattern: String,
    /// 版面密度：`auto` | `comfortable` | `compact`。
    pub density: String,
    /// 裝飾層級：`off` | `auto` | `full`。
    /// `auto` 會依終端機大小自己減量，這是預設也是建議值。
    pub decorations: String,
}

impl Default for Visual {
    fn default() -> Self {
        Self {
            theme: None,
            graph_style: None,
            pattern: "braille".into(),
            density: "auto".into(),
            decorations: "auto".into(),
        }
    }
}

/// `[labels]` —— 區塊別名。
///
/// 別名是**加在**原名前面（`BRAIN · CPU`），不是取代。
/// 換了機器、換了人看，還是知道那是什麼。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Labels {
    pub cpu: String,
    pub gpu: String,
    pub memory: String,
    pub storage: String,
    pub network: String,
    pub process: String,
}

impl Labels {
    /// 這一頁要顯示的標題。有別名就 `別名 · 原名`，沒有就原名。
    pub fn decorate(&self, canonical: &str) -> String {
        let alias = match canonical.to_ascii_lowercase().as_str() {
            "cpu" => &self.cpu,
            "gpu" => &self.gpu,
            "memory" => &self.memory,
            "storage" => &self.storage,
            "network" => &self.network,
            "processes" | "process" => &self.process,
            _ => "",
        };
        if alias.is_empty() {
            canonical.to_owned()
        } else {
            format!("{alias} · {canonical}")
        }
    }

    /// 窄畫面用：只給原名，別名讓位給資料。
    pub fn canonical_only(canonical: &str) -> &str {
        canonical
    }

    fn each_mut(&mut self) -> [(&'static str, &mut String); 6] {
        [
            ("cpu", &mut self.cpu),
            ("gpu", &mut self.gpu),
            ("memory", &mut self.memory),
            ("storage", &mut self.storage),
            ("network", &mut self.network),
            ("process", &mut self.process),
        ]
    }
}

/// 把一個字串限定在允許的選項內，不合法就換成預設值並記一筆。
fn one_of(
    field: &mut String,
    allowed: &[&str],
    fallback: &str,
    what: &str,
    notes: &mut Vec<String>,
) {
    let lower = field.trim().to_ascii_lowercase();
    if allowed.contains(&lower.as_str()) {
        *field = lower;
        return;
    }
    notes.push(format!(
        "{what} 未知值 {field:?}，改用 {fallback}（可用：{}）",
        allowed.join(" / ")
    ));
    *field = fallback.to_owned();
}

/// 品牌代號 / 別名的共用規則：只收 A–Z 0–9，最多 6 字，轉大寫。
///
/// 收窄到英數字是為了版面可預測 —— 別名會排進面板標題，
/// 全形字或控制字元會把框線撐破，那是 v1 踩過的坑。
pub fn normalize_tag(raw: &str, max: usize) -> Result<String, String> {
    let up = raw.trim().to_ascii_uppercase();
    if up.is_empty() {
        return Ok(String::new());
    }
    if let Some(bad) = up.chars().find(|c| !c.is_ascii_alphanumeric()) {
        return Err(format!("{bad:?} 不是英文字母或數字"));
    }
    if up.chars().count() > max {
        return Err(format!("最多 {max} 個字，收到 {}", up.chars().count()));
    }
    Ok(up)
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval: 1.0,
            theme: "default".into(),
            default_page: "overview".into(),
            history: 240,
            temperature_unit: "celsius".into(),
            graph_style: "area".into(),
            show_per_core: true,
            process_sort: "cpu".into(),
            gpu: true,
            branding: Branding::default(),
            visual: Visual::default(),
            labels: Labels::default(),
        }
    }
}

impl Config {
    /// 設定檔路徑。遵循 XDG 規範。
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("sysview").join("config.toml"))
    }

    /// 載入設定。
    ///
    /// **設定檔壞掉不該讓程式起不來** —— 回傳預設值加上一則警告，
    /// 使用者還是看得到系統狀態。
    pub fn load() -> (Self, Option<String>) {
        let Some(path) = Self::path() else {
            return (Self::default(), None);
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => {
                return (
                    Self::default(),
                    Some(format!("讀不到 {}：{e}（使用預設值）", path.display())),
                )
            }
        };
        match toml::from_str::<Self>(&text) {
            Ok(c) => {
                let (c, warn) = c.sanitized();
                (c, warn.map(|w| format!("{}：{w}", path.display())))
            }
            Err(e) => (
                Self::default(),
                Some(format!("{} 格式錯誤：{e}（使用預設值）", path.display())),
            ),
        }
    }

    /// 把不合理的值夾回合法範圍，並回報改了什麼。
    ///
    /// 例如 `interval = 0.001` 會讓 sysview 自己變成負載來源 ——
    /// Availability 要求我們不能讓使用者不小心把伺服器弄垮。
    pub fn sanitized(mut self) -> (Self, Option<String>) {
        let mut notes = Vec::new();
        if !(0.2..=60.0).contains(&self.interval) || !self.interval.is_finite() {
            notes.push(format!("interval {} 超出 0.2–60 秒，已夾住", self.interval));
            self.interval = self.interval.clamp(0.2, 60.0);
            if !self.interval.is_finite() {
                self.interval = 1.0;
            }
        }
        if !crate::theme::THEMES.contains(&self.theme.as_str()) {
            notes.push(format!("未知主題 {:?}，改用 default", self.theme));
            self.theme = "default".into();
        }
        if !(30..=4096).contains(&self.history) {
            notes.push(format!("history {} 超出 30–4096，已夾住", self.history));
            self.history = self.history.clamp(30, 4096);
        }
        if !matches!(self.temperature_unit.as_str(), "celsius" | "fahrenheit") {
            notes.push(format!(
                "未知溫度單位 {:?}，改用 celsius",
                self.temperature_unit
            ));
            self.temperature_unit = "celsius".into();
        }
        if !matches!(self.graph_style.as_str(), "area" | "line") {
            notes.push(format!("未知圖表樣式 {:?}，改用 area", self.graph_style));
            self.graph_style = "area".into();
        }
        // ── [visual] 覆蓋頂層設定（兩種寫法都收，新的優先） ──
        if let Some(t) = self.visual.theme.take() {
            self.theme = t;
            if !crate::theme::THEMES.contains(&self.theme.as_str()) {
                notes.push(format!("[visual] 未知主題 {:?}，改用 default", self.theme));
                self.theme = "default".into();
            }
        }
        if let Some(g) = self.visual.graph_style.take() {
            self.graph_style = g;
            if !matches!(self.graph_style.as_str(), "area" | "line") {
                notes.push(format!(
                    "[visual] 未知圖表樣式 {:?}，改用 area",
                    self.graph_style
                ));
                self.graph_style = "area".into();
            }
        }
        one_of(
            &mut self.visual.pattern,
            &[
                "solid", "shade", "dot", "braille", "line", "hatch", "digital",
            ],
            "braille",
            "[visual] pattern",
            &mut notes,
        );
        one_of(
            &mut self.visual.density,
            &["auto", "comfortable", "compact"],
            "auto",
            "[visual] density",
            &mut notes,
        );
        one_of(
            &mut self.visual.decorations,
            &["off", "auto", "full"],
            "auto",
            "[visual] decorations",
            &mut notes,
        );

        // ── [branding] ──
        match normalize_tag(&self.branding.name, 6) {
            Ok(n) => self.branding.name = n,
            Err(e) => {
                notes.push(format!(
                    "[branding] name 無效（{e}），沿用 sysview 預設品牌"
                ));
                self.branding.name = String::new();
            }
        }
        one_of(
            &mut self.branding.logo,
            &["auto", "pixel", "small", "text", "off"],
            "auto",
            "[branding] logo",
            &mut notes,
        );
        one_of(
            &mut self.branding.mascot,
            &["fox", "deer", "auto", "none"],
            "auto",
            "[branding] mascot",
            &mut notes,
        );
        one_of(
            &mut self.branding.mascot_mode,
            &["reactive", "manual", "idle"],
            "reactive",
            "[branding] mascot_mode",
            &mut notes,
        );
        one_of(
            &mut self.branding.mascot_state,
            crate::ui::visual::mascot::STATE_NAMES,
            "observe",
            "[branding] mascot_state",
            &mut notes,
        );
        // 動畫張數刻意壓在 1–8：這是裝飾，不值得為它多燒 CPU。
        if !(1..=8).contains(&self.branding.mascot_fps) {
            notes.push(format!(
                "[branding] mascot_fps {} 超出 1–8，已夾住",
                self.branding.mascot_fps
            ));
            self.branding.mascot_fps = self.branding.mascot_fps.clamp(1, 8);
        }

        // ── [labels] ──
        for (key, val) in self.labels.each_mut() {
            match normalize_tag(val, 6) {
                Ok(v) => *val = v,
                Err(e) => {
                    notes.push(format!("[labels] {key} 無效（{e}），已忽略"));
                    val.clear();
                }
            }
        }

        let note = (!notes.is_empty()).then(|| notes.join("；"));
        (self, note)
    }

    pub fn interval_duration(&self) -> Duration {
        Duration::from_secs_f64(self.interval)
    }

    /// 產生一份帶註解的預設設定檔，供 `--write-config` 使用。
    pub fn template() -> &'static str {
        r#"# sysview 設定檔  ~/.config/sysview/config.toml
#
# 注意：這個檔案只影響外觀與取樣頻率。
# 管理員權限由 sudo / sudoers 決定，**不可能**在這裡開啟。

# 主更新間隔（秒），有效範圍 0.2–60
interval = 1.0

# 主題：default / high-contrast / catppuccin / tokyo-night / nord / gruvbox / dracula
theme = "default"

# 啟動頁面：overview / cpu / memory / gpu / storage / network / process
default_page = "overview"

# 圖表保留的歷史點數，有效範圍 30–4096
history = 240

# 溫度單位：celsius / fahrenheit
temperature_unit = "celsius"

# 圖表樣式：area（面積圖）/ line（折線圖）
graph_style = "area"

# 總覽是否顯示每個核心的長條
show_per_core = true

# 行程列表預設排序：cpu / rss / pid / thr / user
process_sort = "cpu"

# 是否啟用 GPU 監控。
# 關掉會跳過 NVML 的 dlopen，常駐記憶體少約 15 MB
# （那 15 MB 是 NVIDIA 驅動函式庫自己的緩衝區，不是 sysview 佔用的）。
gpu = true


# ─────────────────────────────────────────────────────────────────────
# 品牌識別
#
# 這一段完全不影響授權。名稱、吉祥物、logo 都只是畫面上的東西，
# 不會、也不可能改變 sudo 能不能過。
# ─────────────────────────────────────────────────────────────────────
[branding]

# 品牌代號。只收 A–Z 與 0–9，最多 6 個字，會自動轉大寫。
# 留空就用 sysview 自己的名字。
# 收得這麼窄是因為代號會排進頁首與面板標題，全形字會把框線撐破。
name = ""

# logo 呈現：auto / pixel / small / text / off
# auto 會依終端機大小自己選；指定 pixel 但畫面放不下時仍然會降級，
# 版面不會因此爆掉。
logo = "auto"

# 吉祥物：fox / deer / auto / none
# auto 會依系統狀態挑 —— 忙的時候是狐狸（動作感），閒的時候是鹿（安靜）。
mascot = "auto"

# 吉祥物的選法：
#   reactive  跟著系統狀態換姿態（閒置→休息、忙碌→前行、吃緊→專注、剛恢復→回望）
#   manual    固定成 mascot_state 指定的姿態
#   idle      只在系統真的閒下來時才出現，其餘時間讓位給資料
mascot_mode = "reactive"

# manual 模式的固定姿態：observe / explore / proceed / rest / return
mascot_state = "observe"

# 動畫張數（每秒），有效範圍 1–8。
# 上限刻意壓得很低：這是裝飾，不值得為它燒 CPU。
# 動畫時鐘跟取樣時鐘是分開的 —— 不管動畫跑多快，
# /proc、NVML、行程掃描的頻率都不會被動到。
mascot_fps = 3

# 啟動時顯示品牌畫面。1.5 秒後自己消失，任何鍵也能立刻跳過。
# 不想要就設 false，或用 --no-splash。
splash = true


# ─────────────────────────────────────────────────────────────────────
# 視覺語言
# ─────────────────────────────────────────────────────────────────────
[visual]

# 組成類長條的填滿紋理：
#   solid / shade / dot / braille / line / hatch / digital
#
# 這不只是好看。顏色在單色終端、8 色 SSH、色盲讀者那裡會失效，
# 紋理是一條獨立的通道 —— 所以 pattern 不會因為關掉裝飾而消失。
pattern = "braille"

# 版面密度：auto / comfortable / compact
density = "auto"

# 裝飾層級：off / auto / full
#   off   完全不畫 logo、吉祥物、紋樣
#   auto  依終端機大小自己減量（建議）
#   full  盡量畫，但畫面真的太小時仍然讓位給資料
decorations = "auto"


# ─────────────────────────────────────────────────────────────────────
# 區塊別名
#
# 別名是**加在**原名前面（BRAIN · CPU），不是取代它。
# 換了機器、換了人看，還是知道那是什麼。
# 規則同品牌代號：A–Z 0–9，最多 6 個字。
# 窄畫面時別名會讓位給原名。
# ─────────────────────────────────────────────────────────────────────
[labels]
cpu = ""
gpu = ""
memory = ""
storage = ""
network = ""
process = ""
"#
    }
}

/// 溫度顯示轉換。
pub fn temperature(celsius: f64, unit: &str) -> (f64, &'static str) {
    if unit == "fahrenheit" {
        (celsius * 9.0 / 5.0 + 32.0, "°F")
    } else {
        (celsius, "°C")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.interval, 1.0);
        assert!(crate::theme::THEMES.contains(&c.theme.as_str()));
        let (c, note) = c.sanitized();
        assert!(note.is_none(), "預設值不該觸發任何修正");
        assert_eq!(c.interval, 1.0);
    }

    #[test]
    fn clamps_dangerously_fast_interval() {
        let c = Config {
            interval: 0.001,
            ..Default::default()
        };
        let (c, note) = c.sanitized();
        assert_eq!(c.interval, 0.2, "過快的取樣會讓 sysview 自己變成負載來源");
        assert!(note.is_some());
    }

    #[test]
    fn clamps_absurd_history_length() {
        let (c, _) = Config {
            history: 10_000_000,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(c.history, 4096, "無上限的歷史會造成記憶體無限成長");
        let (c, _) = Config {
            history: 0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(c.history, 30);
    }

    #[test]
    fn rejects_unknown_theme_gracefully() {
        let (c, note) = Config {
            theme: "hacker".into(),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(c.theme, "default");
        assert!(note.unwrap().contains("hacker"));
    }

    #[test]
    fn handles_non_finite_interval() {
        let (c, _) = Config {
            interval: f64::NAN,
            ..Default::default()
        }
        .sanitized();
        assert!(c.interval.is_finite());
        let (c, _) = Config {
            interval: f64::INFINITY,
            ..Default::default()
        }
        .sanitized();
        assert!(c.interval.is_finite() && c.interval <= 60.0);
    }

    #[test]
    fn config_cannot_grant_privileges() {
        // 這是安全不變量：設定檔裡不可以出現任何與授權有關的欄位
        let toml_text = toml::to_string(&Config::default()).unwrap();
        for forbidden in [
            "admin",
            "root",
            "privileged",
            "sudo",
            "helper_path",
            "allow",
        ] {
            assert!(
                !toml_text.contains(forbidden),
                "設定檔不可包含 {forbidden:?} —— 授權只能由 sudo 決定"
            );
        }
    }

    #[test]
    fn unknown_fields_are_rejected_not_silently_ignored() {
        // deny_unknown_fields：若使用者以為自己開了什麼開關，要明確報錯
        let r: Result<Config, _> = toml::from_str("interval = 1.0\nadmin = true\n");
        assert!(r.is_err(), "未知欄位必須報錯，避免使用者誤以為設定生效了");
    }

    #[test]
    fn template_parses_and_is_valid() {
        let c: Config = toml::from_str(Config::template()).expect("內建範本必須合法");
        let (_, note) = c.sanitized();
        assert!(note.is_none(), "內建範本不該觸發修正：{note:?}");
    }

    #[test]
    fn temperature_conversion() {
        assert_eq!(temperature(100.0, "celsius"), (100.0, "°C"));
        let (f, u) = temperature(100.0, "fahrenheit");
        assert_eq!(u, "°F");
        assert!((f - 212.0).abs() < 1e-9);
    }

    #[test]
    fn config_path_follows_xdg() {
        if let Some(p) = Config::path() {
            let s = p.to_string_lossy();
            assert!(
                s.ends_with("sysview/config.toml"),
                "路徑應符合 XDG 慣例：{s}"
            );
        }
    }
}

#[cfg(test)]
mod visual_config_tests {
    use super::*;

    #[test]
    fn the_printed_template_parses_back_into_the_defaults() {
        // --print-config 印出來的東西，貼回設定檔一定要能用。
        // 印一份自己都讀不了的範本，比不印還糟。
        let parsed: Config = toml::from_str(Config::template()).expect("範本自己要能解析");
        let (parsed, warn) = parsed.sanitized();
        assert_eq!(warn, None, "範本不該觸發任何警告：{warn:?}");
        assert_eq!(parsed.branding, Branding::default());
        assert_eq!(parsed.visual, Visual::default());
        assert_eq!(parsed.labels, Labels::default());
    }

    #[test]
    fn brand_codes_are_narrowed_to_what_a_panel_title_can_hold() {
        assert_eq!(normalize_tag("devlab", 6).unwrap(), "DEVLAB");
        assert_eq!(normalize_tag("  x1  ", 6).unwrap(), "X1");
        assert_eq!(normalize_tag("", 6).unwrap(), "");
        // 全形字會把面板框線撐破 —— v1 就是這樣壞的
        assert!(normalize_tag("實驗室", 6).is_err());
        assert!(normalize_tag("CTW-LAB", 6).is_err());
        assert!(normalize_tag("TOOLONGNAME", 6).is_err());
    }

    #[test]
    fn a_bad_brand_falls_back_instead_of_breaking_the_ui() {
        let c = Config {
            branding: Branding {
                name: "壞掉的名字".into(),
                logo: "nonsense".into(),
                mascot: "cat".into(),
                mascot_mode: "wild".into(),
                mascot_state: "sleeping".into(),
                mascot_fps: 200,
                ..Default::default()
            },
            ..Default::default()
        };
        let (c, warn) = c.sanitized();
        let warn = warn.expect("該有警告");
        assert!(warn.contains("branding"));
        assert_eq!(
            c.branding.name, "",
            "壞掉的品牌要退回預設，不是留著撐破版面"
        );
        assert_eq!(c.branding.logo, "auto");
        assert_eq!(c.branding.mascot, "auto");
        assert_eq!(c.branding.mascot_mode, "reactive");
        assert_eq!(c.branding.mascot_state, "observe");
        assert_eq!(c.branding.mascot_fps, 8, "fps 要被夾在上限，不是照收");
    }

    #[test]
    fn visual_section_can_override_the_older_top_level_spelling() {
        let c: Config = toml::from_str(
            r#"
            theme = "default"
            graph_style = "area"
            [visual]
            theme = "nord"
            graph_style = "line"
            "#,
        )
        .unwrap();
        let (c, warn) = c.sanitized();
        assert_eq!(warn, None);
        assert_eq!(c.theme, "nord", "[visual] 的寫法應該覆蓋頂層");
        assert_eq!(c.graph_style, "line");
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_silently_ignored() {
        // 打錯字卻沒人告訴你，比直接報錯更難查
        for bad in [
            "[branding]\nnmae = \"X\"\n",
            "[visual]\npatern = \"dot\"\n",
            "[labels]\ncpi = \"X\"\n",
        ] {
            assert!(toml::from_str::<Config>(bad).is_err(), "{bad:?} 應該被拒絕");
        }
    }

    #[test]
    fn labels_decorate_without_replacing() {
        let l = Labels {
            cpu: "BRAIN".into(),
            ..Default::default()
        };
        assert_eq!(l.decorate("CPU"), "BRAIN · CPU");
        // 沒設別名就原樣
        assert_eq!(l.decorate("Memory"), "Memory");
        // 認不得的名字也原樣，不會變成空字串
        assert_eq!(l.decorate("Admin"), "Admin");
    }

    #[test]
    fn config_cannot_grant_privileges() {
        // 規格裡的硬性限制：設定檔不得影響授權
        let text = Config::template();
        for forbidden in ["admin", "allow_root", "sudo", "nopasswd", "privileg"] {
            let hits: Vec<&str> = text
                .lines()
                .filter(|l| {
                    let l = l.to_ascii_lowercase();
                    !l.trim_start().starts_with('#') && l.contains(forbidden)
                })
                .collect();
            assert!(
                hits.is_empty(),
                "設定範本裡出現了看起來能改權限的選項 {forbidden:?}：{hits:?}"
            );
        }
    }
}
