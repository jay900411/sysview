//! 響應式版面。
//!
//! 終端機大小差異極大：SSH 進去可能是 80×24，開全螢幕可能是 200×60。
//! 版面必須**優雅降級**而不是畫爛或 panic。

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// 依終端機寬高決定用哪種版面。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    /// ≥132×26：雙欄，資訊最完整
    Wide,
    /// ≥96×30：單欄但全部面板都在
    Tall,
    /// ≥60×16：只保留最重要的面板
    Compact,
    /// 再小就只顯示提示
    TooSmall,
}

impl Density {
    pub fn of(area: Rect) -> Self {
        let (w, h) = (area.width, area.height);
        if w < 50 || h < 12 {
            Self::TooSmall
        } else if w >= 132 && h >= 26 {
            Self::Wide
        } else if w >= 96 && h >= 30 {
            Self::Tall
        } else {
            Self::Compact
        }
    }
}

/// 主畫面切成 header / body / footer。
pub fn chrome(area: Rect) -> (Rect, Rect, Rect) {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    (v[0], v[1], v[2])
}

/// 垂直等分成多塊，可指定比例。
pub fn rows(area: Rect, weights: &[u16]) -> Vec<Rect> {
    if weights.is_empty() {
        return vec![area];
    }
    let cs: Vec<Constraint> = weights.iter().map(|w| Constraint::Fill(*w)).collect();
    Layout::default()
        .direction(Direction::Vertical)
        .constraints(cs)
        .split(area)
        .to_vec()
}

/// 水平等分。
pub fn cols(area: Rect, weights: &[u16]) -> Vec<Rect> {
    if weights.is_empty() {
        return vec![area];
    }
    let cs: Vec<Constraint> = weights.iter().map(|w| Constraint::Fill(*w)).collect();
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(cs)
        .split(area)
        .to_vec()
}

/// 固定高度 + 剩餘。
pub fn split_top(area: Rect, top: u16) -> (Rect, Rect) {
    let top = top.min(area.height);
    (
        Rect {
            height: top,
            ..area
        },
        Rect {
            y: area.y + top,
            height: area.height.saturating_sub(top),
            ..area
        },
    )
}

/// 置中的浮動視窗（Help / Explain / 確認框用）。
pub fn centered(area: Rect, max_w: u16, max_h: u16) -> Rect {
    let w = max_w.min(area.width.saturating_sub(2)).max(1);
    let h = max_h.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// 依可用高度決定一個列表能顯示幾列，並把捲動位置夾在合法範圍。
pub fn clamp_scroll(scroll: usize, total: usize, visible: usize) -> usize {
    let max = total.saturating_sub(visible);
    scroll.min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_classifies_common_terminal_sizes() {
        assert_eq!(Density::of(Rect::new(0, 0, 190, 48)), Density::Wide);
        assert_eq!(Density::of(Rect::new(0, 0, 100, 40)), Density::Tall);
        assert_eq!(Density::of(Rect::new(0, 0, 80, 24)), Density::Compact);
        assert_eq!(Density::of(Rect::new(0, 0, 40, 10)), Density::TooSmall);
        assert_eq!(
            Density::of(Rect::new(0, 0, 200, 8)),
            Density::TooSmall,
            "太矮也算太小"
        );
    }

    #[test]
    fn chrome_never_exceeds_the_area() {
        for h in 3..60u16 {
            let area = Rect::new(0, 0, 80, h);
            let (a, b, c) = chrome(area);
            assert_eq!(a.height + b.height + c.height, h);
            assert!(c.y + c.height <= area.y + area.height);
        }
    }

    #[test]
    fn centered_fits_inside_small_areas() {
        for (w, h) in [(10u16, 5u16), (80, 24), (200, 60), (3, 3)] {
            let area = Rect::new(0, 0, w, h);
            let c = centered(area, 84, 30);
            assert!(
                c.x + c.width <= area.x + area.width,
                "{w}x{h} 的置中視窗超出右邊界"
            );
            assert!(
                c.y + c.height <= area.y + area.height,
                "{w}x{h} 的置中視窗超出下邊界"
            );
            assert!(c.width >= 1 && c.height >= 1);
        }
    }

    #[test]
    fn split_top_handles_oversized_request() {
        let area = Rect::new(0, 0, 40, 10);
        let (top, rest) = split_top(area, 100);
        assert_eq!(top.height, 10);
        assert_eq!(rest.height, 0, "要求超過總高度時不可下溢");
    }

    #[test]
    fn rows_and_cols_partition_exactly() {
        let area = Rect::new(0, 0, 100, 40);
        let r = rows(area, &[1, 1, 2]);
        assert_eq!(r.len(), 3);
        assert_eq!(r.iter().map(|x| x.height).sum::<u16>(), 40);
        let c = cols(area, &[1, 1]);
        assert_eq!(c.iter().map(|x| x.width).sum::<u16>(), 100);
    }

    #[test]
    fn empty_weights_return_whole_area() {
        let area = Rect::new(0, 0, 10, 10);
        assert_eq!(rows(area, &[]), vec![area]);
        assert_eq!(cols(area, &[]), vec![area]);
    }

    #[test]
    fn scroll_is_clamped_to_content() {
        assert_eq!(clamp_scroll(1000, 20, 10), 10);
        assert_eq!(clamp_scroll(5, 20, 10), 5);
        assert_eq!(clamp_scroll(5, 3, 10), 0, "內容比畫面少時不該能捲動");
        assert_eq!(clamp_scroll(usize::MAX / 2, 100, 25), 75);
    }
}
