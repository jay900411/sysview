//! 各頁面共用的繪製元件。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::metrics::model::{Reading, Series, Severity};
use crate::theme::Theme;
use crate::ui::format;
use crate::ui::widgets::braille::{BrailleGraph, GraphStyle};
use crate::ui::widgets::gauge::LabeledGauge;

/// 「鍵 : 值」兩欄表格，是詳細頁面最常見的排版。
///
/// 每一列都會登記成一個可選的子節點，所以使用者可以往下鑽進面板、
/// 選到「Logical CPUs」「Core clock」這種**單一數字**再按 e 看說明。
/// 只到面板層的話，說明會比寫死的清單還粗。
pub struct KeyValues<'a> {
    rows: Vec<Row>,
    pub key_width: u16,
    pub theme: &'a Theme,
    /// 這張表所屬的面板索引。有值才會把每一列登記成可選節點。
    parent: Option<(&'a crate::app::App, usize)>,
}

struct Row {
    key: String,
    value: String,
    style: Option<Style>,
    /// 這一列對應哪個 metric。`None` 代表這列沒有說明可看。
    metric_id: Option<&'static str>,
}

impl<'a> KeyValues<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self {
            rows: Vec::new(),
            key_width: 18,
            theme,
            parent: None,
        }
    }
    /// 把每一列登記成 `panel` 底下的子節點。
    pub fn focusable(mut self, app: &'a crate::app::App, panel: usize) -> Self {
        if panel != usize::MAX {
            self.parent = Some((app, panel));
        }
        self
    }
    pub fn key_width(mut self, w: u16) -> Self {
        self.key_width = w;
        self
    }
    pub fn row(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.rows.push(Row {
            key: k.into(),
            value: v.into(),
            style: None,
            metric_id: None,
        });
        self
    }
    /// 加一列並指定它對應的 metric，這樣選到它按 e 才有說明。
    pub fn row_of(
        mut self,
        k: impl Into<String>,
        v: impl Into<String>,
        metric_id: &'static str,
    ) -> Self {
        self.rows.push(Row {
            key: k.into(),
            value: v.into(),
            style: None,
            metric_id: Some(metric_id),
        });
        self
    }
    pub fn styled_row(mut self, k: impl Into<String>, v: impl Into<String>, s: Style) -> Self {
        self.rows.push(Row {
            key: k.into(),
            value: v.into(),
            style: Some(s),
            metric_id: None,
        });
        self
    }
    pub fn styled_row_of(
        mut self,
        k: impl Into<String>,
        v: impl Into<String>,
        s: Style,
        metric_id: &'static str,
    ) -> Self {
        self.rows.push(Row {
            key: k.into(),
            value: v.into(),
            style: Some(s),
            metric_id: Some(metric_id),
        });
        self
    }
    /// 直接放一個 Reading：自動處理 n/a 與 estimated 標記，
    /// 並沿用 Reading 自己帶的 metric id 當說明來源。
    pub fn reading(mut self, k: impl Into<String>, r: &Reading) -> Self {
        let style = if r.quality.is_available() {
            None
        } else {
            Some(self.theme.faint_style())
        };
        self.rows.push(Row {
            key: k.into(),
            value: format::reading(r),
            style,
            metric_id: r.metric_id,
        });
        self
    }
}

impl Widget for KeyValues<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let vstyle = self.theme.style(self.theme.palette.fg);
        for (i, row) in self.rows.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }
            let kw = self.key_width.min(area.width);
            buf.set_string(
                area.x,
                y,
                format::pad(&row.key, kw as usize),
                self.theme.dim_style(),
            );
            let vx = area.x + kw + 1;
            if vx < area.x + area.width {
                let w = (area.x + area.width).saturating_sub(vx) as usize;
                buf.set_string(
                    vx,
                    y,
                    format::truncate(&row.value, w),
                    row.style.unwrap_or(vstyle),
                );
            }
            // 登記成可選的一列，label 就是畫面上看到的那個鍵名。
            // 鍵名空白的列（例如同一介面的第二、第三個 IP）不登記 ——
            // 一個沒有名稱的可選項既選不明白，焦點列也顯示不出東西。
            if row.key.trim().is_empty() {
                continue;
            }
            if let (Some((app, parent)), Some(id)) = (self.parent, row.metric_id) {
                app.regions.add_item(
                    parent,
                    Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    },
                    id,
                    row.key.clone(),
                );
            }
        }
    }
}

/// 畫一條「標籤 + 長條 + 數值」。
#[allow(clippy::too_many_arguments)]
pub fn gauge_row(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    label: &str,
    reading: &Reading,
    percent: Option<f64>,
    value_text: &str,
    style: Option<Style>,
    label_w: u16,
    value_w: u16,
) {
    let available = reading.quality.is_available() && percent.is_some();
    let mut g =
        LabeledGauge::new(label, value_text, percent.unwrap_or(0.0)).widths(label_w, value_w);
    if !available {
        g = g.unavailable();
    }
    if let Some(s) = style {
        g = g.style(s);
    }
    g.render(area, buf, theme);
}

/// 畫一張歷史圖。資料不足時顯示提示而不是空白。
#[allow(clippy::too_many_arguments)]
pub fn history_graph(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    series: &Series,
    color: ratatui::style::Color,
    max: Option<f64>,
    axis_fmt: fn(f64) -> String,
    style: GraphStyle,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if series.len() < 2 {
        let msg = "取樣中…";
        let x = area.x + area.width.saturating_sub(format::width(msg) as u16) / 2;
        buf.set_string(x, area.y + area.height / 2, msg, theme.faint_style());
        return;
    }
    let data: Vec<f64> = series.iter().collect();
    let mut g = BrailleGraph::new(&data)
        .style(Style::default().fg(color))
        .graph_style(style)
        .axis(true, theme.faint_style(), axis_fmt);
    if let Some(m) = max {
        g = g.max(m);
    }
    g.render(area, buf);
}

/// 狀態圓點 + 文字。**永遠符號與顏色並存**，色盲使用者才讀得懂。
pub fn status_badge(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    theme: &Theme,
    sev: Severity,
    text: &str,
) -> u16 {
    let style = theme.severity_style(sev);
    buf.set_string(x, y, sev.symbol(), style);
    let tx = x + 2;
    buf.set_string(tx, y, text, style);
    tx + format::width(text) as u16
}

/// 表格表頭。
pub fn table_header(buf: &mut Buffer, area: Rect, theme: &Theme, cols: &[(&str, u16, bool)]) {
    use ratatui::style::Modifier;
    let style = theme.dim_style().add_modifier(Modifier::UNDERLINED);
    let mut x = area.x;
    for (name, w, right) in cols {
        if x >= area.x + area.width {
            break;
        }
        let avail = (area.x + area.width).saturating_sub(x).min(*w) as usize;
        let text = if *right {
            format::pad_left(name, avail)
        } else {
            format::pad(name, avail)
        };
        buf.set_string(x, area.y, text, style);
        x += w + 1;
    }
}

/// 畫一列表格資料。
pub fn table_row(buf: &mut Buffer, area: Rect, cells: &[(String, u16, bool, Style)]) {
    let mut x = area.x;
    for (text, w, right, style) in cells {
        if x >= area.x + area.width {
            break;
        }
        let avail = (area.x + area.width).saturating_sub(x).min(*w) as usize;
        let t = if *right {
            format::pad_left(text, avail)
        } else {
            format::pad(text, avail)
        };
        buf.set_string(x, area.y, t, *style);
        x += w + 1;
    }
}

/// 「這個資料還沒好」的統一顯示。
pub fn placeholder(buf: &mut Buffer, area: Rect, theme: &Theme, text: &str) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let w = format::width(text) as u16;
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height / 2;
    buf.set_string(
        x,
        y,
        format::truncate(text, area.width as usize),
        theme.faint_style(),
    );
}

/// 畫一個面板，並把它登記成可選區域。
///
/// 這是「看到什麼就選什麼」的入口：`label` 就是使用者在框線上看到的字，
/// 焦點列不會另外編一份對不上的名詞。
pub fn focus_panel(
    app: &crate::app::App,
    area: Rect,
    buf: &mut Buffer,
    title: &str,
    subtitle: Option<String>,
    metric_id: &'static str,
) -> (Rect, usize) {
    let mut p = crate::ui::widgets::panel::Panel::new(&app.theme, title);
    if let Some(s) = subtitle {
        p = p.subtitle(s);
    }
    let inner = p.render(area, buf);
    let idx = app.regions.add_panel(area, metric_id, title);
    (inner, idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Unavailable;
    use crate::metrics::model::Unit;
    use crate::theme::ColorDepth;

    fn t() -> Theme {
        Theme::new("default", ColorDepth::TrueColor)
    }

    #[test]
    fn key_values_truncates_instead_of_overflowing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        KeyValues::new(&t())
            .row("非常長的鍵名稱會被裁掉", "非常長的值也會被裁掉")
            .render(Rect::new(0, 0, 30, 5), &mut buf);
        // 最後一欄之外不該被寫到
        assert_eq!(buf.cell((29, 1)).unwrap().symbol(), " ");
    }

    #[test]
    fn unavailable_reading_renders_as_text_not_zero() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 3));
        let r = Reading::unavailable(Unit::Celsius, Unavailable::Unsupported);
        KeyValues::new(&t())
            .reading("Temp", &r)
            .render(Rect::new(0, 0, 40, 3), &mut buf);
        let row = crate::ui::buffer_row(&buf, 0);
        assert!(row.contains("unsupported"), "得到：{row}");
        assert!(!row.contains("0.0"), "拿不到的溫度不可顯示成 0");
    }

    #[test]
    fn status_badge_includes_symbol() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 1));
        status_badge(&mut buf, 0, 0, &t(), Severity::Critical, "disk full");
        assert_eq!(
            buf.cell((0, 0)).unwrap().symbol(),
            Severity::Critical.symbol()
        );
    }

    #[test]
    fn history_graph_shows_hint_when_data_is_short() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 5));
        let s = Series::new(100);
        history_graph(
            &mut buf,
            Rect::new(0, 0, 40, 5),
            &t(),
            &s,
            ratatui::style::Color::Blue,
            Some(100.0),
            |v| format!("{v:.0}"),
            GraphStyle::Area,
        );
        let text = crate::ui::buffer_row(&buf, 2);
        assert!(
            text.contains("取樣中"),
            "資料不足時應提示而不是留白：{text}"
        );
    }

    #[test]
    fn all_renderers_survive_zero_sized_areas() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 5));
        let th = t();
        let zero = Rect::new(0, 0, 0, 0);
        placeholder(&mut buf, zero, &th, "x");
        history_graph(
            &mut buf,
            zero,
            &th,
            &Series::new(10),
            ratatui::style::Color::Red,
            None,
            |v| format!("{v}"),
            GraphStyle::Area,
        );
        table_header(&mut buf, zero, &th, &[("A", 5, false)]);
        KeyValues::new(&th).row("a", "b").render(zero, &mut buf);
    }

    #[test]
    fn table_header_and_row_align() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 2));
        let th = t();
        let cols = [("PID", 7u16, true), ("USER", 10, false)];
        table_header(&mut buf, Rect::new(0, 0, 40, 1), &th, &cols);
        table_row(
            &mut buf,
            Rect::new(0, 1, 40, 1),
            &[
                ("1234".into(), 7, true, th.dim_style()),
                ("alice".into(), 10, false, th.dim_style()),
            ],
        );
        let h: String = crate::ui::buffer_row(&buf, 0).chars().take(7).collect();
        let r: String = crate::ui::buffer_row(&buf, 1).chars().take(7).collect();
        assert_eq!(h.trim_end(), "    PID");
        assert_eq!(r.trim_end(), "   1234", "數字欄應靠右對齊，才好比較大小");
    }
}
