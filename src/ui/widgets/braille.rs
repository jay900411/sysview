//! 點陣（Braille）面積圖。
//!
//! 一個 Unicode braille 字元是 **2 欄 × 4 列**的點陣，所以同樣的終端機面積
//! 能塞進 8 倍解析度。這是 btop / bottom 那種「這真的是 terminal？」觀感的來源。
//!
//! Ratatui 內建的 `Sparkline` 一格只有 8 階，`Chart` 又太重，所以自己寫。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

/// braille 點的位元對應。
///
/// ```text
/// 點編號   位元值
///  1  4    0x01 0x08
///  2  5    0x02 0x10
///  3  6    0x04 0x20
///  7  8    0x40 0x80
/// ```
/// 索引方式：`DOTS[欄(0..2)][列(0..4)]`
pub const DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
const BRAILLE_BASE: u32 = 0x2800;

/// 圖表的畫法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphStyle {
    /// 從底部填滿到數值（適合使用率）
    Area,
    /// 只畫輪廓線（適合速率，比較不會糊成一片）
    Line,
}

pub struct BrailleGraph<'a> {
    data: &'a [f64],
    style: Style,
    graph: GraphStyle,
    /// 固定的 Y 軸上限。`None` = 依資料自動縮放。
    max: Option<f64>,
    /// 是否在右側畫刻度
    axis: bool,
    axis_style: Style,
    /// 刻度的格式化函式
    axis_fmt: fn(f64) -> String,
}

impl<'a> BrailleGraph<'a> {
    pub fn new(data: &'a [f64]) -> Self {
        Self {
            data,
            style: Style::default(),
            graph: GraphStyle::Area,
            max: None,
            axis: false,
            axis_style: Style::default(),
            axis_fmt: |v| format!("{v:.0}"),
        }
    }
    pub fn style(mut self, s: Style) -> Self {
        self.style = s;
        self
    }
    pub fn graph_style(mut self, g: GraphStyle) -> Self {
        self.graph = g;
        self
    }
    pub fn max(mut self, m: f64) -> Self {
        self.max = Some(m);
        self
    }
    pub fn axis(mut self, on: bool, style: Style, fmt: fn(f64) -> String) -> Self {
        self.axis = on;
        self.axis_style = style;
        self.axis_fmt = fmt;
        self
    }
}

/// 把資料算成 braille 位元圖。抽出來是為了能單獨測試。
///
/// 回傳 `rows × cols` 的位元陣列，每個位元組是一個 braille 字元的點陣。
pub fn rasterize(data: &[f64], cols: usize, rows: usize, max: f64, style: GraphStyle) -> Vec<u8> {
    let mut grid = vec![0u8; rows * cols];
    if cols == 0 || rows == 0 || data.is_empty() {
        return grid;
    }
    let dot_cols = cols * 2;
    let dot_rows = rows * 4;
    let max = if max > 0.0 && max.is_finite() {
        max
    } else {
        1e-9
    };

    // 只畫得下最後 dot_cols 個點；靠右對齊，讓最新的資料在右邊
    let start = data.len().saturating_sub(dot_cols);
    let visible = &data[start..];
    let offset = dot_cols - visible.len();

    for (i, &v) in visible.iter().enumerate() {
        let col = offset + i;
        if col >= dot_cols {
            break;
        }
        let v = if v.is_finite() { v } else { 0.0 };
        let ratio = (v / max).clamp(0.0, 1.0);
        let level = (ratio * dot_rows as f64).round() as usize;
        // 有值但不到一個點高時，仍畫最底下一個點 —— 否則小流量會完全看不見
        let level = if level == 0 && v > 0.0 { 1 } else { level };
        if level == 0 {
            continue;
        }
        let top_dot = dot_rows - level;
        let range: Box<dyn Iterator<Item = usize>> = match style {
            GraphStyle::Area => Box::new(top_dot..dot_rows),
            GraphStyle::Line => Box::new(std::iter::once(top_dot)),
        };
        for dot_row in range {
            if dot_row >= dot_rows {
                continue;
            }
            let (cy, ry) = (dot_row / 4, dot_row % 4);
            let (cx, rx) = (col / 2, col % 2);
            if cy < rows && cx < cols {
                grid[cy * cols + cx] |= DOTS[rx][ry];
            }
        }
    }
    grid
}

/// 位元組 → braille 字元。
pub fn braille_char(bits: u8) -> char {
    char::from_u32(BRAILLE_BASE + bits as u32).unwrap_or(' ')
}

impl Widget for BrailleGraph<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        // 右側留 6 格畫刻度
        let axis_w = if self.axis && area.width > 14 { 6 } else { 0 };
        let cols = area.width.saturating_sub(axis_w) as usize;
        let rows = area.height as usize;
        if cols == 0 {
            return;
        }

        let max = self.max.unwrap_or_else(|| {
            self.data
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(0.0_f64, f64::max)
        });
        let grid = rasterize(self.data, cols, rows, max, self.graph);

        for y in 0..rows {
            for x in 0..cols {
                let bits = grid[y * cols + x];
                if bits == 0 {
                    continue; // 空白格不畫，讓背景透出來
                }
                if let Some(cell) = buf.cell_mut((area.x + x as u16, area.y + y as u16)) {
                    cell.set_char(braille_char(bits)).set_style(self.style);
                }
            }
        }

        if axis_w > 0 {
            let ax = area.x + cols as u16 + 1;
            let labels = [
                (0u16, max),
                (area.height / 2, max / 2.0),
                (area.height.saturating_sub(1), 0.0),
            ];
            for (dy, val) in labels {
                if dy >= area.height {
                    continue;
                }
                let text = (self.axis_fmt)(val);
                let text = crate::ui::format::pad_left(&text, (axis_w - 1) as usize);
                buf.set_string(ax, area.y + dy, text, self.axis_style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_data_produces_blank_grid() {
        let g = rasterize(&[], 10, 3, 100.0, GraphStyle::Area);
        assert_eq!(g.len(), 30);
        assert!(g.iter().all(|&b| b == 0));
    }

    #[test]
    fn zero_dimensions_do_not_panic() {
        assert!(rasterize(&[1.0, 2.0], 0, 3, 10.0, GraphStyle::Area).is_empty());
        assert!(rasterize(&[1.0, 2.0], 5, 0, 10.0, GraphStyle::Area).is_empty());
    }

    #[test]
    fn full_value_fills_entire_column() {
        // 單一欄位、滿值 → 該欄所有列都要有點
        let g = rasterize(&[100.0, 100.0], 1, 2, 100.0, GraphStyle::Area);
        assert_eq!(g.len(), 2);
        assert!(g.iter().all(|&b| b != 0), "滿值應填滿整欄");
    }

    #[test]
    fn zero_value_draws_nothing() {
        let g = rasterize(&[0.0, 0.0, 0.0, 0.0], 2, 2, 100.0, GraphStyle::Area);
        assert!(g.iter().all(|&b| b == 0), "0 不該畫出任何點");
    }

    #[test]
    fn tiny_nonzero_value_still_visible() {
        // 0.1% 在 4 列的圖上四捨五入會是 0 列，但仍該看得見
        let g = rasterize(&[0.1], 1, 1, 100.0, GraphStyle::Area);
        assert!(g.iter().any(|&b| b != 0), "微小但非零的值不該完全消失");
    }

    #[test]
    fn area_fills_more_than_line() {
        let data: Vec<f64> = (0..20).map(|i| i as f64 * 5.0).collect();
        let area: u32 = rasterize(&data, 10, 4, 100.0, GraphStyle::Area)
            .iter()
            .map(|b| b.count_ones())
            .sum();
        let line: u32 = rasterize(&data, 10, 4, 100.0, GraphStyle::Line)
            .iter()
            .map(|b| b.count_ones())
            .sum();
        assert!(area > line, "面積圖的點數應多於折線圖");
        assert!(line > 0);
    }

    #[test]
    fn data_is_right_aligned_so_newest_is_on_the_right() {
        // 只有一個資料點、寬 5 格（=10 個點欄）→ 應該畫在最右邊
        let g = rasterize(&[100.0], 5, 1, 100.0, GraphStyle::Area);
        assert_eq!(g[0], 0, "最左邊應是空的");
        assert_ne!(g[4], 0, "最新的資料應在最右邊");
    }

    #[test]
    fn oversized_data_keeps_only_the_newest() {
        let data: Vec<f64> = (0..1000)
            .map(|i| if i < 990 { 0.0 } else { 100.0 })
            .collect();
        let g = rasterize(&data, 3, 1, 100.0, GraphStyle::Area);
        // 只有最後 6 個點欄可見，那些都是 100
        assert!(g.iter().all(|&b| b != 0), "應只保留最新的資料");
    }

    #[test]
    fn non_finite_values_are_treated_as_zero() {
        let g = rasterize(
            &[f64::NAN, f64::INFINITY, 0.0],
            2,
            1,
            100.0,
            GraphStyle::Area,
        );
        assert!(g.iter().all(|&b| b == 0), "NaN/Inf 不可造成 panic 或亂畫");
    }

    #[test]
    fn zero_max_does_not_divide_by_zero() {
        let g = rasterize(&[5.0, 10.0], 4, 2, 0.0, GraphStyle::Area);
        assert_eq!(g.len(), 8);
    }

    #[test]
    fn values_above_max_are_clamped() {
        let g = rasterize(&[500.0], 1, 2, 100.0, GraphStyle::Area);
        assert!(g.iter().all(|&b| b != 0), "超出上限應被夾住而不是溢位");
    }

    #[test]
    fn braille_chars_are_in_the_correct_unicode_block() {
        assert_eq!(braille_char(0x00), '\u{2800}');
        assert_eq!(braille_char(0xFF), '\u{28FF}');
        for b in 0..=255u8 {
            let c = braille_char(b);
            assert!(
                ('\u{2800}'..='\u{28FF}').contains(&c),
                "0x{b:02x} 產生了非 braille 字元 {c:?}"
            );
        }
    }

    #[test]
    fn braille_chars_are_single_width() {
        use unicode_width::UnicodeWidthChar;
        for b in [0u8, 0x01, 0x7f, 0xff] {
            assert_eq!(
                UnicodeWidthChar::width(braille_char(b)),
                Some(1),
                "braille 必須是半形，否則圖表會撐破框線"
            );
        }
    }

    #[test]
    fn renders_into_buffer_without_overflow() {
        let area = Rect::new(2, 1, 20, 4);
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 8));
        let data: Vec<f64> = (0..60).map(|i| (i as f64 * 7.0) % 100.0).collect();
        BrailleGraph::new(&data).max(100.0).render(area, &mut buf);
        // 區域外不該被寫到
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), " ");
        assert_eq!(buf.cell((29, 7)).unwrap().symbol(), " ");
    }

    #[test]
    fn tiny_area_does_not_panic() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        for (w, h) in [(0, 0), (1, 1), (0, 5), (5, 0), (2, 1)] {
            BrailleGraph::new(&[1.0, 2.0, 3.0])
                .max(10.0)
                .render(Rect::new(0, 0, w, h), &mut buf);
        }
    }
}
