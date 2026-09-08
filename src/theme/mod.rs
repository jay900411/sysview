//! Theme 抽象層。
//!
//! # 為什麼不把顏色寫死在 widget 裡
//!
//! 每個 widget 都應該說「我要畫的是 CPU 的強調色」，而不是「我要畫 39 號色」。
//! 這樣換主題時不用動 widget，加新主題也只是多一組數值。
//!
//! # 色盲友善
//!
//! 紅綠色盲約佔男性人口 8%。因此本專案有一條硬性規則：
//!
//! > **絕不只用顏色傳達嚴重程度。**
//!
//! 每個 [`Severity`] 都配一個獨立符號（`●◐▲■○`），critical 狀態另外附
//! `CRIT` / `WARN` 文字。把整個畫面轉成灰階仍然讀得懂。

use ratatui::style::{Color, Modifier, Style};

use crate::metrics::model::Severity;

/// 一整套顏色。全部用 24-bit RGB 指定，再依終端機能力降級。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: Color,
    pub surface: Color,
    pub fg: Color,
    pub dim: Color,
    pub faint: Color,
    pub border: Color,
    pub accent: Color,
    pub ok: Color,
    pub notice: Color,
    pub warning: Color,
    pub critical: Color,
    pub cpu: Color,
    pub memory: Color,
    pub gpu: Color,
    pub disk: Color,
    pub network: Color,
    pub process: Color,
    /// 估計值 / 不可用資料的顏色，刻意低調
    pub estimated: Color,
}

/// 終端機的顏色能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 24-bit RGB
    TrueColor,
    /// 256 色索引
    Indexed256,
    /// 只有 8/16 基本色
    Basic,
    /// 完全沒有顏色（管線、`NO_COLOR`、dumb terminal）
    Monochrome,
}

impl ColorDepth {
    /// 偵測終端機能力。尊重 `NO_COLOR` 標準（https://no-color.org）。
    pub fn detect() -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            return Self::Monochrome;
        }
        let term = std::env::var("TERM").unwrap_or_default();
        if term.is_empty() || term == "dumb" {
            return Self::Monochrome;
        }
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            return Self::TrueColor;
        }
        if term.contains("256")
            || term.contains("kitty")
            || term.contains("alacritty")
            || term.contains("ghostty")
            || term.contains("wezterm")
        {
            return Self::Indexed256;
        }
        Self::Basic
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub palette: Palette,
    pub depth: ColorDepth,
}

impl Theme {
    pub fn new(name: &str, depth: ColorDepth) -> Self {
        let (name, palette) = named(name);
        Self {
            name,
            palette,
            depth,
        }
    }

    /// 把調色盤的顏色依終端機能力降級。
    pub fn c(&self, color: Color) -> Color {
        match self.depth {
            ColorDepth::TrueColor => color,
            ColorDepth::Indexed256 => downgrade_256(color),
            ColorDepth::Basic => downgrade_basic(color),
            ColorDepth::Monochrome => Color::Reset,
        }
    }

    pub fn style(&self, color: Color) -> Style {
        Style::default().fg(self.c(color))
    }
    pub fn bold(&self, color: Color) -> Style {
        self.style(color).add_modifier(Modifier::BOLD)
    }
    pub fn dim_style(&self) -> Style {
        // 沒有顏色時改用 DIM 修飾子，仍能表現層次
        match self.depth {
            ColorDepth::Monochrome => Style::default().add_modifier(Modifier::DIM),
            _ => self.style(self.palette.dim),
        }
    }
    pub fn faint_style(&self) -> Style {
        match self.depth {
            ColorDepth::Monochrome => Style::default().add_modifier(Modifier::DIM),
            _ => self.style(self.palette.faint),
        }
    }
    /// 常駐浮水印用的樣式：比 faint 再退一階。
    ///
    /// 浮水印是背景不是內容 —— 掃視資料時它不能把視線拉走。faint 已經是
    /// 調色盤裡最淡的顏色，再加 DIM 讓它退到剛好看得出形狀的程度。
    /// 這台終端機有沒有顏色可用。沒有的話只能用反白 / DIM 表達層次。
    pub fn depth_is_monochrome(&self) -> bool {
        self.depth == ColorDepth::Monochrome
    }
    pub fn watermark_style(&self) -> Style {
        self.faint_style().add_modifier(Modifier::DIM)
    }
    /// Overlay 視窗（說明頁、Explain、恐龍、啟動畫面）的底色。
    ///
    /// 視窗一定要**自己畫底色**。`Clear` 之後格子的背景是 `Reset`，也就是
    /// 終端機自己的預設底色 —— 在深色終端機上看不出差別，但 macOS 的
    /// Terminal.app 預設 profile 是白底：整個視窗變成白框、淺灰的字看不見
    /// （使用者回報「大曝光」）。儀表板本身在 `draw()` 開頭就塗了 `bg`，
    /// 只有 overlay 漏了。單色模式下 `c()` 回 `Reset`，那是刻意交給終端機。
    pub fn surface_style(&self) -> Style {
        Style::default().bg(self.c(self.palette.surface))
    }
    pub fn border_style(&self) -> Style {
        self.style(self.palette.border)
    }
    pub fn title_style(&self) -> Style {
        self.bold(self.palette.accent)
    }

    /// 嚴重程度對應的顏色。
    pub fn severity_color(&self, s: Severity) -> Color {
        let p = &self.palette;
        self.c(match s {
            Severity::Ok => p.ok,
            Severity::Notice => p.notice,
            Severity::Warning => p.warning,
            Severity::Critical => p.critical,
            Severity::Unknown => p.faint,
        })
    }

    /// 嚴重程度對應的樣式。
    ///
    /// **critical / warning 會額外加上粗體**，這樣即使在單色終端機
    /// 或色盲使用者眼中也能分辨。
    pub fn severity_style(&self, s: Severity) -> Style {
        let base = Style::default().fg(self.severity_color(s));
        match s {
            Severity::Critical => base.add_modifier(Modifier::BOLD),
            Severity::Warning => base.add_modifier(Modifier::BOLD),
            _ => base,
        }
    }

    /// 百分比 → 樣式。
    pub fn gauge_style(&self, percent: f64) -> Style {
        self.severity_style(Severity::from_percent(percent))
    }

    /// 各子系統的代表色。
    pub fn subsystem(&self, s: Subsystem) -> Color {
        let p = &self.palette;
        self.c(match s {
            Subsystem::Cpu => p.cpu,
            Subsystem::Memory => p.memory,
            Subsystem::Gpu => p.gpu,
            Subsystem::Disk => p.disk,
            Subsystem::Network => p.network,
            Subsystem::Process => p.process,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    Cpu,
    Memory,
    Gpu,
    Disk,
    Network,
    Process,
}

/// 全部內建主題的名稱，供 `--theme` 與設定檔使用。
pub const THEMES: &[&str] = &[
    "default",
    "high-contrast",
    "catppuccin",
    "tokyo-night",
    "nord",
    "gruvbox",
    "dracula",
];

fn named(name: &str) -> (&'static str, Palette) {
    match name.to_ascii_lowercase().as_str() {
        "high-contrast" | "hc" => ("high-contrast", HIGH_CONTRAST),
        "catppuccin" | "catppuccin-mocha" => ("catppuccin", CATPPUCCIN),
        "tokyo-night" | "tokyonight" => ("tokyo-night", TOKYO_NIGHT),
        "nord" => ("nord", NORD),
        "gruvbox" | "gruvbox-dark" => ("gruvbox", GRUVBOX),
        "dracula" => ("dracula", DRACULA),
        _ => ("default", DEFAULT),
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// 預設主題。中性、偏冷色，長時間盯著不刺眼。
pub const DEFAULT: Palette = Palette {
    bg: rgb(0x0d, 0x11, 0x17),
    surface: rgb(0x16, 0x1b, 0x22),
    fg: rgb(0xc9, 0xd1, 0xd9),
    dim: rgb(0x8b, 0x94, 0x9e),
    faint: rgb(0x56, 0x5f, 0x6a),
    border: rgb(0x30, 0x36, 0x3d),
    accent: rgb(0x58, 0xa6, 0xff),
    ok: rgb(0x3f, 0xb9, 0x50),
    notice: rgb(0x58, 0xa6, 0xff),
    warning: rgb(0xd2, 0x99, 0x22),
    critical: rgb(0xf8, 0x51, 0x49),
    cpu: rgb(0x58, 0xa6, 0xff),
    memory: rgb(0xbc, 0x8c, 0xff),
    gpu: rgb(0x3f, 0xb9, 0x50),
    disk: rgb(0xdb, 0x6d, 0x28),
    network: rgb(0x39, 0xc5, 0xcf),
    process: rgb(0xe3, 0xb3, 0x41),
    estimated: rgb(0x76, 0x80, 0x8a),
};

/// 高對比主題。給投影機、強光環境，或視力需求較高的使用者。
pub const HIGH_CONTRAST: Palette = Palette {
    bg: rgb(0x00, 0x00, 0x00),
    surface: rgb(0x0a, 0x0a, 0x0a),
    fg: rgb(0xff, 0xff, 0xff),
    dim: rgb(0xd0, 0xd0, 0xd0),
    faint: rgb(0x90, 0x90, 0x90),
    border: rgb(0xa0, 0xa0, 0xa0),
    accent: rgb(0x00, 0xd7, 0xff),
    ok: rgb(0x00, 0xff, 0x87),
    notice: rgb(0x00, 0xd7, 0xff),
    warning: rgb(0xff, 0xd7, 0x00),
    critical: rgb(0xff, 0x00, 0x5f),
    cpu: rgb(0x00, 0xd7, 0xff),
    memory: rgb(0xd7, 0x87, 0xff),
    gpu: rgb(0x5f, 0xff, 0x5f),
    disk: rgb(0xff, 0xaf, 0x00),
    network: rgb(0x5f, 0xff, 0xff),
    process: rgb(0xff, 0xff, 0x5f),
    estimated: rgb(0xb0, 0xb0, 0xb0),
};

pub const CATPPUCCIN: Palette = Palette {
    bg: rgb(0x1e, 0x1e, 0x2e),
    surface: rgb(0x31, 0x32, 0x44),
    fg: rgb(0xcd, 0xd6, 0xf4),
    dim: rgb(0xa6, 0xad, 0xc8),
    faint: rgb(0x6c, 0x70, 0x86),
    border: rgb(0x45, 0x47, 0x5a),
    accent: rgb(0x89, 0xb4, 0xfa),
    ok: rgb(0xa6, 0xe3, 0xa1),
    notice: rgb(0x89, 0xdc, 0xeb),
    warning: rgb(0xf9, 0xe2, 0xaf),
    critical: rgb(0xf3, 0x8b, 0xa8),
    cpu: rgb(0x89, 0xb4, 0xfa),
    memory: rgb(0xcb, 0xa6, 0xf7),
    gpu: rgb(0xa6, 0xe3, 0xa1),
    disk: rgb(0xfa, 0xb3, 0x87),
    network: rgb(0x94, 0xe2, 0xd5),
    process: rgb(0xf9, 0xe2, 0xaf),
    estimated: rgb(0x7f, 0x84, 0x9c),
};

pub const TOKYO_NIGHT: Palette = Palette {
    bg: rgb(0x1a, 0x1b, 0x26),
    surface: rgb(0x24, 0x28, 0x3b),
    fg: rgb(0xc0, 0xca, 0xf5),
    dim: rgb(0x9a, 0xa5, 0xce),
    faint: rgb(0x56, 0x5f, 0x89),
    border: rgb(0x3b, 0x42, 0x61),
    accent: rgb(0x7a, 0xa2, 0xf7),
    ok: rgb(0x9e, 0xce, 0x6a),
    notice: rgb(0x7d, 0xcf, 0xff),
    warning: rgb(0xe0, 0xaf, 0x68),
    critical: rgb(0xf7, 0x76, 0x8e),
    cpu: rgb(0x7a, 0xa2, 0xf7),
    memory: rgb(0xbb, 0x9a, 0xf7),
    gpu: rgb(0x9e, 0xce, 0x6a),
    disk: rgb(0xff, 0x9e, 0x64),
    network: rgb(0x2a, 0xc3, 0xde),
    process: rgb(0xe0, 0xaf, 0x68),
    estimated: rgb(0x6b, 0x70, 0x89),
};

pub const NORD: Palette = Palette {
    bg: rgb(0x2e, 0x34, 0x40),
    surface: rgb(0x3b, 0x42, 0x52),
    fg: rgb(0xd8, 0xde, 0xe9),
    dim: rgb(0xa6, 0xb1, 0xc2),
    faint: rgb(0x66, 0x71, 0x85),
    border: rgb(0x4c, 0x56, 0x6a),
    accent: rgb(0x88, 0xc0, 0xd0),
    ok: rgb(0xa3, 0xbe, 0x8c),
    notice: rgb(0x81, 0xa1, 0xc1),
    warning: rgb(0xeb, 0xcb, 0x8b),
    critical: rgb(0xbf, 0x61, 0x6a),
    cpu: rgb(0x81, 0xa1, 0xc1),
    memory: rgb(0xb4, 0x8e, 0xad),
    gpu: rgb(0xa3, 0xbe, 0x8c),
    disk: rgb(0xd0, 0x87, 0x70),
    network: rgb(0x8f, 0xbc, 0xbb),
    process: rgb(0xeb, 0xcb, 0x8b),
    estimated: rgb(0x77, 0x82, 0x96),
};

pub const GRUVBOX: Palette = Palette {
    bg: rgb(0x28, 0x28, 0x28),
    surface: rgb(0x3c, 0x38, 0x36),
    fg: rgb(0xeb, 0xdb, 0xb2),
    dim: rgb(0xbd, 0xae, 0x93),
    faint: rgb(0x7c, 0x6f, 0x64),
    border: rgb(0x50, 0x49, 0x45),
    accent: rgb(0x83, 0xa5, 0x98),
    ok: rgb(0xb8, 0xbb, 0x26),
    notice: rgb(0x83, 0xa5, 0x98),
    warning: rgb(0xfa, 0xbd, 0x2f),
    critical: rgb(0xfb, 0x49, 0x34),
    cpu: rgb(0x83, 0xa5, 0x98),
    memory: rgb(0xd3, 0x86, 0x9b),
    gpu: rgb(0xb8, 0xbb, 0x26),
    disk: rgb(0xfe, 0x80, 0x19),
    network: rgb(0x8e, 0xc0, 0x7c),
    process: rgb(0xfa, 0xbd, 0x2f),
    estimated: rgb(0x92, 0x83, 0x74),
};

pub const DRACULA: Palette = Palette {
    bg: rgb(0x28, 0x2a, 0x36),
    surface: rgb(0x44, 0x47, 0x5a),
    fg: rgb(0xf8, 0xf8, 0xf2),
    dim: rgb(0xbd, 0xbd, 0xd0),
    faint: rgb(0x6d, 0x6f, 0x84),
    border: rgb(0x51, 0x54, 0x6b),
    accent: rgb(0xbd, 0x93, 0xf9),
    ok: rgb(0x50, 0xfa, 0x7b),
    notice: rgb(0x8b, 0xe9, 0xfd),
    warning: rgb(0xf1, 0xfa, 0x8c),
    critical: rgb(0xff, 0x55, 0x55),
    cpu: rgb(0x8b, 0xe9, 0xfd),
    memory: rgb(0xbd, 0x93, 0xf9),
    gpu: rgb(0x50, 0xfa, 0x7b),
    disk: rgb(0xff, 0xb8, 0x6c),
    network: rgb(0x8b, 0xe9, 0xfd),
    process: rgb(0xf1, 0xfa, 0x8c),
    estimated: rgb(0x7d, 0x80, 0x99),
};

/// RGB → 256 色索引（xterm 的 6×6×6 色立方 + 灰階漸層）。
fn downgrade_256(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else { return c };
    // 接近灰階時用專屬的灰階區段，顏色比較準
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 16 {
        let level = (max as u16 * 23 / 255) as u8;
        return Color::Indexed(232 + level);
    }
    let q = |v: u8| (v as u16 * 5 / 255) as u8;
    Color::Indexed(16 + 36 * q(r) + 6 * q(g) + q(b))
}

/// RGB → 基本 8/16 色。取最接近的色相。
fn downgrade_basic(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else { return c };
    let bright = r.max(g).max(b) > 160;
    let (r, g, b) = (r > 96, g > 96, b > 96);
    match (r, g, b) {
        (true, false, false) => {
            if bright {
                Color::LightRed
            } else {
                Color::Red
            }
        }
        (false, true, false) => {
            if bright {
                Color::LightGreen
            } else {
                Color::Green
            }
        }
        (false, false, true) => {
            if bright {
                Color::LightBlue
            } else {
                Color::Blue
            }
        }
        (true, true, false) => {
            if bright {
                Color::LightYellow
            } else {
                Color::Yellow
            }
        }
        (true, false, true) => {
            if bright {
                Color::LightMagenta
            } else {
                Color::Magenta
            }
        }
        (false, true, true) => {
            if bright {
                Color::LightCyan
            } else {
                Color::Cyan
            }
        }
        (true, true, true) => {
            if bright {
                Color::White
            } else {
                Color::Gray
            }
        }
        (false, false, false) => Color::DarkGray,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_named_themes_resolve() {
        for name in THEMES {
            let t = Theme::new(name, ColorDepth::TrueColor);
            assert_eq!(t.name, *name, "主題 {name} 名稱對不上");
        }
    }

    #[test]
    fn unknown_theme_falls_back_to_default() {
        let t = Theme::new("nonexistent-theme", ColorDepth::TrueColor);
        assert_eq!(t.name, "default");
    }

    #[test]
    fn severity_never_relies_on_color_alone() {
        // 每個嚴重程度都要有獨立符號 —— 這是色盲使用者唯一的線索
        let sevs = [
            Severity::Ok,
            Severity::Notice,
            Severity::Warning,
            Severity::Critical,
            Severity::Unknown,
        ];
        let symbols: Vec<&str> = sevs.iter().map(|s| s.symbol()).collect();
        let uniq: std::collections::HashSet<_> = symbols.iter().collect();
        assert_eq!(uniq.len(), sevs.len(), "符號必須互不相同");
        let labels: std::collections::HashSet<&str> = sevs.iter().map(|s| s.label()).collect();
        assert_eq!(labels.len(), sevs.len(), "文字標籤也必須互不相同");
    }

    #[test]
    fn critical_and_warning_are_bold_even_without_color() {
        let t = Theme::new("default", ColorDepth::Monochrome);
        assert!(
            t.severity_style(Severity::Critical)
                .add_modifier
                .contains(Modifier::BOLD),
            "單色終端機下 critical 必須靠粗體區分"
        );
        assert!(t
            .severity_style(Severity::Warning)
            .add_modifier
            .contains(Modifier::BOLD));
    }

    #[test]
    fn monochrome_emits_no_color() {
        let t = Theme::new("catppuccin", ColorDepth::Monochrome);
        assert_eq!(t.c(rgb(255, 0, 0)), Color::Reset, "單色模式不可送出顏色碼");
    }

    #[test]
    fn truecolor_passes_rgb_through_unchanged() {
        let t = Theme::new("default", ColorDepth::TrueColor);
        assert_eq!(t.c(rgb(0x58, 0xa6, 0xff)), rgb(0x58, 0xa6, 0xff));
    }

    #[test]
    fn downgrades_to_valid_256_indices() {
        for c in [
            rgb(0, 0, 0),
            rgb(255, 255, 255),
            rgb(0x58, 0xa6, 0xff),
            rgb(128, 128, 128),
        ] {
            match downgrade_256(c) {
                Color::Indexed(i) => assert!(i >= 16, "應落在色立方或灰階區段，得到 {i}"),
                other => panic!("預期 Indexed，得到 {other:?}"),
            }
        }
    }

    #[test]
    fn downgrades_to_basic_colors() {
        assert_eq!(downgrade_basic(rgb(255, 0, 0)), Color::LightRed);
        assert_eq!(downgrade_basic(rgb(0, 100, 0)), Color::Green);
        assert_eq!(downgrade_basic(rgb(10, 10, 10)), Color::DarkGray);
    }

    #[test]
    fn no_color_env_forces_monochrome() {
        // 尊重 https://no-color.org 標準
        let saved = std::env::var_os("NO_COLOR");
        // SAFETY: 測試為單執行緒環境下的環境變數操作
        unsafe { std::env::set_var("NO_COLOR", "1") };
        assert_eq!(ColorDepth::detect(), ColorDepth::Monochrome);
        unsafe {
            match saved {
                Some(v) => std::env::set_var("NO_COLOR", v),
                None => std::env::remove_var("NO_COLOR"),
            }
        }
    }

    #[test]
    fn every_palette_has_distinct_subsystem_colors() {
        for name in THEMES {
            let t = Theme::new(name, ColorDepth::TrueColor);
            let subs = [
                Subsystem::Cpu,
                Subsystem::Memory,
                Subsystem::Gpu,
                Subsystem::Disk,
                Subsystem::Network,
                Subsystem::Process,
            ];
            let colors: Vec<Color> = subs.iter().map(|s| t.subsystem(*s)).collect();
            let uniq: std::collections::HashSet<_> =
                colors.iter().map(|c| format!("{c:?}")).collect();
            // network 與 cpu 在少數主題會相同，允許最多一組重複
            assert!(
                uniq.len() >= colors.len() - 1,
                "{name} 的子系統顏色過於接近"
            );
        }
    }
}
