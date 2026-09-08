//! 面板外框：圓角邊框 + 標題 + 靠右副標題。
//!
//! 統一在這裡處理，所有頁面的視覺語言才會一致。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use crate::theme::Theme;
use crate::ui::format;

pub struct Panel<'a> {
    title: &'a str,
    subtitle: Option<String>,
    theme: &'a Theme,
    title_style: Option<Style>,
    /// 強調外框（例如診斷面板要用警示色）
    border_style: Option<Style>,
}

impl<'a> Panel<'a> {
    pub fn new(theme: &'a Theme, title: &'a str) -> Self {
        Self {
            title,
            subtitle: None,
            theme,
            title_style: None,
            border_style: None,
        }
    }
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }
    pub fn title_style(mut self, s: Style) -> Self {
        self.title_style = Some(s);
        self
    }
    pub fn border_style(mut self, s: Style) -> Self {
        self.border_style = Some(s);
        self
    }

    /// 畫外框，回傳可用的內容區。
    pub fn render(self, area: Rect, buf: &mut Buffer) -> Rect {
        if area.width < 2 || area.height < 2 {
            return Rect::new(area.x, area.y, 0, 0);
        }
        let border = self
            .border_style
            .unwrap_or_else(|| self.theme.border_style());
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border)
            .render(area, buf);

        // 標題嵌在上緣：╭─┤ TITLE ├───
        let tstyle = self.title_style.unwrap_or_else(|| self.theme.title_style());
        let mut x = area.x + 2;
        let max_title = area.width.saturating_sub(6) as usize;
        let title = format::truncate(self.title, max_title);
        if !title.is_empty() {
            buf.set_string(x, area.y, "┤ ", border);
            x += 2;
            buf.set_string(x, area.y, &title, tstyle);
            x += format::width(&title) as u16;
            buf.set_string(x, area.y, " ├", border);
        }

        // 副標題靠右：───┤ SUB ├╮
        if let Some(sub) = &self.subtitle {
            let used = (x - area.x) as usize + 2;
            let budget = (area.width as usize).saturating_sub(used + 8);
            let sub = format::truncate(sub, budget);
            let sw = format::width(&sub);
            if sw > 0 {
                let sx = area.x + area.width.saturating_sub(5 + sw as u16);
                if sx > x {
                    buf.set_string(sx, area.y, "┤ ", border);
                    buf.set_string(sx + 2, area.y, &sub, self.theme.faint_style());
                    buf.set_string(sx + 2 + sw as u16, area.y, " ├", border);
                }
            }
        }
        Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        }
    }
}

/// 內容區再往內縮一格，讓文字不貼著邊框。
pub fn inset(area: Rect, h: u16, v: u16) -> Rect {
    Rect {
        x: area.x + h,
        y: area.y + v,
        width: area.width.saturating_sub(h * 2),
        height: area.height.saturating_sub(v * 2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorDepth;

    fn theme() -> Theme {
        Theme::new("default", ColorDepth::TrueColor)
    }

    #[test]
    fn returns_inner_area_inside_border() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
        let inner = Panel::new(&theme(), "CPU").render(Rect::new(0, 0, 40, 10), &mut buf);
        assert_eq!(inner, Rect::new(1, 1, 38, 8));
    }

    #[test]
    fn draws_rounded_corners() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 5));
        Panel::new(&theme(), "X").render(Rect::new(0, 0, 20, 5), &mut buf);
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "╭");
        assert_eq!(buf.cell((19, 0)).unwrap().symbol(), "╮");
        assert_eq!(buf.cell((0, 4)).unwrap().symbol(), "╰");
        assert_eq!(buf.cell((19, 4)).unwrap().symbol(), "╯");
    }

    #[test]
    fn cjk_title_does_not_break_the_border() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 4));
        Panel::new(&theme(), "記憶體")
            .subtitle("62.5 GB")
            .render(Rect::new(0, 0, 30, 4), &mut buf);
        // 右上角必須還在 —— v1 就是這裡因為用 strlen 算寬度而被蓋掉
        assert_eq!(
            buf.cell((29, 0)).unwrap().symbol(),
            "╮",
            "全形標題不可覆蓋右上角"
        );
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), "╭");
    }

    #[test]
    fn long_title_is_truncated_not_overflowed() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));
        Panel::new(&theme(), "這是一個非常非常長的標題會超出邊框")
            .render(Rect::new(0, 0, 20, 4), &mut buf);
        assert_eq!(buf.cell((19, 0)).unwrap().symbol(), "╮");
    }

    #[test]
    fn subtitle_is_dropped_when_there_is_no_room() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 14, 4));
        Panel::new(&theme(), "CPU")
            .subtitle("這個副標題放不下")
            .render(Rect::new(0, 0, 14, 4), &mut buf);
        assert_eq!(
            buf.cell((13, 0)).unwrap().symbol(),
            "╮",
            "放不下時應捨棄副標題而不是撐破"
        );
    }

    #[test]
    fn degenerate_areas_return_empty_and_do_not_panic() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 10));
        for (w, h) in [(0, 0), (1, 1), (1, 5), (5, 1), (2, 2)] {
            let inner = Panel::new(&theme(), "t").render(Rect::new(0, 0, w, h), &mut buf);
            assert!(inner.width <= w.saturating_sub(2));
        }
    }

    #[test]
    fn inset_never_underflows() {
        let a = inset(Rect::new(0, 0, 3, 3), 5, 5);
        assert_eq!(a.width, 0);
        assert_eq!(a.height, 0);
    }
}
