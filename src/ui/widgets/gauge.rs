//! 漸層長條與量表。
//!
//! 用 1/8 格的區塊字元（`▏▎▍▌▋▊▉█`）做次像素填充，所以 20 格寬的長條
//! 實際有 160 階解析度，看起來比整格跳動的長條平順很多。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::metrics::model::Severity;
use crate::theme::Theme;

/// 1/8 格的區塊字元。索引 0 是空白，8 是滿格。
pub const BLOCKS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
/// 未填滿部分的底紋。用點而不是空白，長條的範圍才看得出來。
pub const TRACK: char = '·';

/// 把百分比換算成「幾個滿格 + 一個部分格」。
///
/// 回傳 `(滿格數, 部分格的 1/8 階數 0..=8)`。
pub fn fill_cells(percent: f64, width: usize) -> (usize, usize) {
    if width == 0 {
        return (0, 0);
    }
    let p = if percent.is_finite() {
        percent.clamp(0.0, 100.0)
    } else {
        0.0
    };
    let exact = p / 100.0 * width as f64;
    let full = exact.floor() as usize;
    let frac = exact - full as f64;
    let eighths = (frac * 8.0).round() as usize;
    // 邊界：四捨五入到 8 就等於進位成一個滿格
    if eighths >= 8 && full < width {
        (full + 1, 0)
    } else {
        (full.min(width), if full >= width { 0 } else { eighths })
    }
}

pub struct Gauge<'a> {
    percent: f64,
    style: Option<Style>,
    theme: &'a Theme,
    /// 有值嗎。`false` 時整條畫成底紋並顯示 n/a。
    available: bool,
}

impl<'a> Gauge<'a> {
    pub fn new(theme: &'a Theme, percent: f64) -> Self {
        Self {
            percent,
            style: None,
            theme,
            available: true,
        }
    }
    pub fn unavailable(theme: &'a Theme) -> Self {
        Self {
            percent: 0.0,
            style: None,
            theme,
            available: false,
        }
    }
    /// 覆寫顏色（子系統色）；不指定就依嚴重程度自動上色。
    pub fn style(mut self, s: Style) -> Self {
        self.style = Some(s);
        self
    }
}

impl Widget for Gauge<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let w = area.width as usize;
        let track = self.theme.faint_style();
        if !self.available {
            let s: String = std::iter::repeat_n(TRACK, w).collect();
            buf.set_string(area.x, area.y, s, track);
            return;
        }
        let fill_style = self
            .style
            .unwrap_or_else(|| self.theme.gauge_style(self.percent));
        let (full, eighths) = fill_cells(self.percent, w);

        let mut s = String::with_capacity(w * 3);
        for _ in 0..full {
            s.push(BLOCKS[8]);
        }
        let mut used = full;
        if used < w && eighths > 0 {
            s.push(BLOCKS[eighths]);
            used += 1;
        }
        buf.set_string(area.x, area.y, &s, fill_style);
        if used < w {
            let rest: String = std::iter::repeat_n(TRACK, w - used).collect();
            buf.set_string(area.x + used as u16, area.y, rest, track);
        }
    }
}

/// 一列「標籤 + 長條 + 數值」，是整個 UI 最常出現的元件。
pub struct LabeledGauge<'a> {
    pub label: &'a str,
    pub value_text: &'a str,
    pub percent: f64,
    pub available: bool,
    pub severity: Option<Severity>,
    pub label_width: u16,
    pub value_width: u16,
    pub style: Option<Style>,
}

impl<'a> LabeledGauge<'a> {
    pub fn new(label: &'a str, value_text: &'a str, percent: f64) -> Self {
        Self {
            label,
            value_text,
            percent,
            available: true,
            severity: None,
            label_width: 10,
            value_width: 12,
            style: None,
        }
    }
    pub fn unavailable(mut self) -> Self {
        self.available = false;
        self
    }
    pub fn widths(mut self, label: u16, value: u16) -> Self {
        self.label_width = label;
        self.value_width = value;
        self
    }
    pub fn style(mut self, s: Style) -> Self {
        self.style = Some(s);
        self
    }
    pub fn severity(mut self, s: Severity) -> Self {
        self.severity = Some(s);
        self
    }

    pub fn render(self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        use crate::ui::format;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let mut x = area.x;
        let lw = self.label_width.min(area.width);
        buf.set_string(
            x,
            area.y,
            format::pad(self.label, lw as usize),
            theme.dim_style(),
        );
        x += lw + 1;
        if x >= area.x + area.width {
            return;
        }

        // 嚴重程度符號：色盲使用者唯一的線索，永遠與顏色並存
        let sev = self.severity.unwrap_or_else(|| {
            if self.available {
                Severity::from_percent(self.percent)
            } else {
                Severity::Unknown
            }
        });
        let remaining = (area.x + area.width).saturating_sub(x);
        let vw = self.value_width.min(remaining);
        let bar_w = remaining.saturating_sub(vw + 3);

        if bar_w > 0 {
            let g = if self.available {
                match self.style {
                    Some(s) => Gauge::new(theme, self.percent).style(s),
                    None => Gauge::new(theme, self.percent),
                }
            } else {
                Gauge::unavailable(theme)
            };
            g.render(
                Rect {
                    x,
                    y: area.y,
                    width: bar_w,
                    height: 1,
                },
                buf,
            );
            x += bar_w + 1;
        }
        // 符號
        if x < area.x + area.width {
            buf.set_string(x, area.y, sev.symbol(), theme.severity_style(sev));
            x += 2;
        }
        if x < area.x + area.width {
            let w = (area.x + area.width).saturating_sub(x) as usize;
            buf.set_string(
                x,
                area.y,
                format::pad_left(self.value_text, w),
                if self.available {
                    theme.style(theme.palette.fg)
                } else {
                    theme.faint_style()
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorDepth;

    #[test]
    fn fill_boundaries_are_exact() {
        assert_eq!(fill_cells(0.0, 10), (0, 0));
        assert_eq!(fill_cells(100.0, 10), (10, 0));
        assert_eq!(fill_cells(50.0, 10), (5, 0));
    }

    #[test]
    fn fractional_fill_uses_eighth_blocks() {
        // 10 格寬、55% → 5 個滿格 + 4/8 格
        let (full, eighths) = fill_cells(55.0, 10);
        assert_eq!(full, 5);
        assert_eq!(eighths, 4);
    }

    #[test]
    fn never_exceeds_width() {
        for p in [
            -50.0,
            0.0,
            33.3,
            99.9,
            100.0,
            150.0,
            f64::NAN,
            f64::INFINITY,
        ] {
            for w in [0, 1, 5, 20, 100] {
                let (full, e) = fill_cells(p, w);
                assert!(full <= w, "{p} 在 {w} 格中產生了 {full} 個滿格");
                assert!(e <= 8);
                assert!(full + usize::from(e > 0) <= w.max(1), "總格數超出寬度");
            }
        }
    }

    #[test]
    fn non_finite_percent_renders_as_empty() {
        assert_eq!(fill_cells(f64::NAN, 10), (0, 0));
        assert_eq!(fill_cells(f64::INFINITY, 10), (0, 0));
    }

    #[test]
    fn zero_width_is_safe() {
        assert_eq!(fill_cells(50.0, 0), (0, 0));
    }

    #[test]
    fn block_chars_are_single_width() {
        use unicode_width::UnicodeWidthChar;
        for c in BLOCKS {
            assert_eq!(
                UnicodeWidthChar::width(c),
                Some(1),
                "{c:?} 不是半形，會撐破長條"
            );
        }
        assert_eq!(UnicodeWidthChar::width(TRACK), Some(1));
    }

    #[test]
    fn gauge_fills_exactly_the_given_area() {
        let theme = Theme::new("default", ColorDepth::TrueColor);
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 3));
        Gauge::new(&theme, 50.0).render(Rect::new(2, 1, 10, 1), &mut buf);
        // 區域左右各一格應保持空白
        assert_eq!(buf.cell((1, 1)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((12, 1)).unwrap().symbol(), " ");
        // 前半應是滿格
        assert_eq!(buf.cell((2, 1)).unwrap().symbol(), "█");
        // 後半應是底紋
        assert_eq!(buf.cell((11, 1)).unwrap().symbol(), "·");
    }

    #[test]
    fn unavailable_gauge_shows_only_track() {
        let theme = Theme::new("default", ColorDepth::TrueColor);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        Gauge::unavailable(&theme).render(Rect::new(0, 0, 10, 1), &mut buf);
        for x in 0..10 {
            assert_eq!(
                buf.cell((x, 0)).unwrap().symbol(),
                "·",
                "拿不到資料時不可畫出任何填充，否則看起來像 0%"
            );
        }
    }

    #[test]
    fn labeled_gauge_includes_severity_symbol() {
        let theme = Theme::new("default", ColorDepth::TrueColor);
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 1));
        LabeledGauge::new("RAM", "95%", 95.0).render(Rect::new(0, 0, 60, 1), &mut buf, &theme);
        let row = crate::ui::buffer_row(&buf, 0);
        assert!(
            row.contains(Severity::Critical.symbol()),
            "critical 狀態必須有符號，不能只靠紅色：{row}"
        );
    }

    #[test]
    fn tiny_areas_do_not_panic() {
        let theme = Theme::new("default", ColorDepth::TrueColor);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        for w in 0..10u16 {
            LabeledGauge::new("x", "1", 50.0).render(Rect::new(0, 0, w, 1), &mut buf, &theme);
            Gauge::new(&theme, 50.0).render(Rect::new(0, 0, w, 1), &mut buf);
        }
    }
}
