//! 這次改版新增的視覺化元件。
//!
//! 共同的規矩：
//!
//! * 每一個都先問「畫不下的時候怎麼辦」，答案不能是「畫出去」。
//! * 一律不配置超過必要的記憶體，畫一格就是寫一格。
//! * 顏色只是加分，形狀本身要能讀 —— 單色終端下不能變成一團一樣的東西。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::metrics::model::Severity;
use crate::theme::Theme;
use crate::ui::format;
use crate::ui::visual::pattern::Pattern;

// ─────────────────────────────────────────────────────────────────────────
// Chip / badge
// ─────────────────────────────────────────────────────────────────────────

/// 一個小標籤，像 `[ SSD ]`、`[ UP ]`。
///
/// 用方括號而不是純文字，是為了在一堆數字裡一眼分出「這是分類不是量測值」。
pub fn chip(buf: &mut Buffer, x: u16, y: u16, max_w: u16, text: &str, style: Style) -> u16 {
    let label = format!("[{}]", text);
    let w = format::width(&label) as u16;
    if w > max_w || max_w == 0 {
        return 0;
    }
    buf.set_string(x, y, &label, style);
    w
}

/// 一排 chip，放不下的就不畫（不是截斷 —— 半個 chip 讀起來像壞掉）。
pub fn chips(buf: &mut Buffer, area: Rect, items: &[(String, Style)]) -> u16 {
    let mut x = area.x;
    for (text, style) in items {
        let remain = (area.x + area.width).saturating_sub(x);
        let used = chip(buf, x, area.y, remain, text, *style);
        if used == 0 {
            break;
        }
        x += used + 1;
    }
    x - area.x
}

// ─────────────────────────────────────────────────────────────────────────
// Heatmap
// ─────────────────────────────────────────────────────────────────────────

/// 每個核心一格的熱度圖。
///
/// 24 核以上的機器用逐條長條列會佔掉整頁；熱度圖把同樣的資訊壓成幾列，
/// 而且「哪幾顆在忙」變成一眼可見的空間分佈，那是長條列給不了的。
///
/// 強度同時用**顏色**與**字元密度**編碼，所以單色終端下也讀得出來。
pub fn heatmap(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    values: &[Option<f64>],
    pattern: Pattern,
    cell_w: u16,
) -> u16 {
    if area.width == 0 || area.height == 0 || values.is_empty() {
        return 0;
    }
    let cell_w = cell_w.max(1);
    let per_row = (area.width / cell_w).max(1) as usize;
    let rows = values.len().div_ceil(per_row).min(area.height as usize);

    for (i, v) in values.iter().enumerate() {
        let (r, c) = (i / per_row, i % per_row);
        if r >= rows {
            break;
        }
        let x = area.x + c as u16 * cell_w;
        let y = area.y + r as u16;
        let (ch, style) = match v {
            // 拿不到值就畫成「不可用」，不要拿 0 假裝閒置
            None => ('·', theme.style(theme.palette.estimated)),
            Some(p) => {
                let frac = (p / 100.0).clamp(0.0, 1.0);
                let sev = if *p >= 90.0 {
                    Severity::Critical
                } else if *p >= 70.0 {
                    Severity::Warning
                } else if *p >= 40.0 {
                    Severity::Notice
                } else {
                    Severity::Ok
                };
                (pattern.at(frac), theme.severity_style(sev))
            }
        };
        let mut s = String::with_capacity(cell_w as usize);
        for _ in 0..cell_w.saturating_sub(1).max(1) {
            s.push(ch);
        }
        buf.set_string(x, y, format::truncate(&s, cell_w as usize), style);
    }
    rows as u16
}

// ─────────────────────────────────────────────────────────────────────────
// Waffle
// ─────────────────────────────────────────────────────────────────────────

/// Waffle chart 的一段。
pub struct Slice {
    pub label: String,
    pub value: f64,
    pub pattern: Pattern,
    pub style: Style,
}

/// 比例格圖。
///
/// 選 waffle 而不是 treemap，是因為 treemap 要在整數格上切矩形，
/// 小分區會被捨入成 0 寬或 0 高而整個消失 —— 在終端機這種粗格線的畫布上
/// 那是常態不是例外。waffle 的每一格都是同樣大小，
/// 「這個人佔了幾格」直接可數，捨入誤差最多就是一格。
///
/// 回傳實際用掉的列數。
/// 格圖裡一格用的字元。
///
/// 實心紋樣用 `▇`（下七分之八）而不是 `█`：每一格上緣留一條細縫，兩列
/// 實心格才分得開 —— 不然一個人佔 87% 的時候，兩列連成一整塊色塊，
/// 看不出那是格圖（使用者回報）。細縫只有八分之一格，不用多花一列。
/// 其他紋樣本來就不是實心，列與列之間看得出來，照舊。
pub fn waffle_glyph(p: Pattern) -> char {
    match p.glyph() {
        '█' => '▇',
        c => c,
    }
}

pub fn waffle(buf: &mut Buffer, area: Rect, slices: &[Slice], empty: Style) -> u16 {
    if area.width == 0 || area.height == 0 {
        return 0;
    }
    let cols = area.width as usize;
    let rows = area.height as usize;
    let total_cells = cols * rows;
    let total: f64 = slices.iter().map(|s| s.value.max(0.0)).sum();
    if total <= 0.0 {
        return 0;
    }

    // 先幫每一段預留一格，剩下的才按比例分。
    //
    // 事後才補「至少一格」是不夠的：一個佔 99.9% 的人會先把全部格子吃光，
    // 後面的人怎麼補都補不到位置。用量很小的人也必須看得到自己，
    // 這是 User Storage 這個畫面存在的意義。
    let n = slices.len();
    if n > total_cells {
        // 格子比人還少，那就只能畫得下的那幾個，硬擠只會全部糊掉
        return 0;
    }
    let spare = total_cells - n;
    let mut plan: Vec<(char, Style)> = vec![('·', empty); total_cells];
    let mut cell = 0usize;
    let mut acc = 0.0;
    for (i, s) in slices.iter().enumerate() {
        acc += s.value.max(0.0);
        // 已經分掉的保底格數 + 按比例分到的餘量
        let proportional = ((acc / total) * spare as f64).round() as usize;
        let end = (i + 1 + proportional).min(total_cells);
        let end = end.max(cell + 1);
        for c in plan.iter_mut().take(end).skip(cell) {
            *c = (waffle_glyph(s.pattern), s.style);
        }
        cell = end;
    }
    // 比例捨入後可能還剩幾格，補給最後一段，免得尾端留下空洞
    if cell < total_cells {
        if let Some(last) = slices.last() {
            for c in plan.iter_mut().skip(cell) {
                *c = (waffle_glyph(last.pattern), last.style);
            }
        }
    }

    for r in 0..rows {
        for (x, c) in (area.x..area.x + area.width).zip(0..cols) {
            let (ch, st) = plan[r * cols + c];
            buf.set_string(x, area.y + r as u16, ch.to_string(), st);
        }
    }
    rows as u16
}

// ─────────────────────────────────────────────────────────────────────────
// Mirrored waveform
// ─────────────────────────────────────────────────────────────────────────

/// 上下對映的雙向波形，給下載 / 上傳用。
///
/// 兩條分開的圖要讀者自己在腦裡對齊時間軸；鏡像之後同一個時刻的收與送
/// 就在同一欄上下相對，「這是雙向的還是單向的」變成一眼可見。
///
/// 用 1/8 高度的方塊字元，所以一列能表達 8 階。
pub fn mirrored(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    up: &[f64],
    down: &[f64],
    up_style: Style,
    down_style: Style,
) {
    if area.width == 0 || area.height < 2 {
        return;
    }
    const UPPER: [char; 9] = [' ', '▔', '▔', '▀', '▀', '▀', '█', '█', '█'];
    const LOWER: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    let half = (area.height / 2).max(1);
    let w = area.width as usize;
    let peak = up
        .iter()
        .chain(down.iter())
        .cloned()
        .fold(0.0_f64, f64::max)
        .max(1.0);

    let take = |data: &[f64], i: usize| -> f64 {
        // 靠右對齊：最新的資料在右邊，跟其他圖一致
        if data.len() >= w {
            data[data.len() - w + i]
        } else if i + data.len() >= w {
            data[i + data.len() - w]
        } else {
            0.0
        }
    };

    for i in 0..w {
        let x = area.x + i as u16;
        // 上半：下載，由中線往上長
        let d = (take(down, i) / peak).clamp(0.0, 1.0) * half as f64 * 8.0;
        for r in 0..half {
            let level = (d - (half - 1 - r) as f64 * 8.0).clamp(0.0, 8.0) as usize;
            let ch = UPPER[level];
            if ch != ' ' {
                buf.set_string(x, area.y + r, ch.to_string(), down_style);
            }
        }
        // 下半：上傳，由中線往下長
        let u = (take(up, i) / peak).clamp(0.0, 1.0) * half as f64 * 8.0;
        for r in 0..half {
            let level = (u - r as f64 * 8.0).clamp(0.0, 8.0) as usize;
            let ch = LOWER[level];
            if ch != ' ' {
                buf.set_string(x, area.y + half + r, ch.to_string(), up_style);
            }
        }
    }
    // 中線：沒有它就看不出來哪裡是零
    let mid = area.y + half;
    if mid < area.y + area.height && half * 2 < area.height {
        let line = "─".repeat(w);
        buf.set_string(area.x, mid, line, theme.faint_style());
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Mini strip
// ─────────────────────────────────────────────────────────────────────────

/// 表格列裡的極短強度條。
///
/// 只有幾格寬，目的不是讀出精確值（旁邊就有數字了），
/// 而是讓眼睛能在幾十列裡「掃」出哪幾列偏高。
pub fn strip(buf: &mut Buffer, x: u16, y: u16, width: u16, theme: &Theme, percent: Option<f64>) {
    if width == 0 {
        return;
    }
    let Some(p) = percent else {
        buf.set_string(
            x,
            y,
            "·".repeat(width as usize),
            theme.style(theme.palette.estimated),
        );
        return;
    };
    let frac = (p / 100.0).clamp(0.0, 1.0);
    let filled = (frac * width as f64).round() as usize;
    let sev = if p >= 90.0 {
        Severity::Critical
    } else if p >= 70.0 {
        Severity::Warning
    } else if p >= 40.0 {
        Severity::Notice
    } else {
        Severity::Ok
    };
    let mut s = String::with_capacity(width as usize);
    for i in 0..width as usize {
        s.push(if i < filled { '▰' } else { '▱' });
    }
    buf.set_string(x, y, s, theme.severity_style(sev));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorDepth;
    use ratatui::style::Color;

    fn t() -> Theme {
        Theme::new("default", ColorDepth::TrueColor)
    }
    fn buf(w: u16, h: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, w, h))
    }
    fn text(b: &Buffer) -> String {
        crate::ui::buffer_text(b)
    }

    #[test]
    fn nothing_draws_outside_its_area() {
        // 版面爆掉是這次改版最大的風險
        for (w, h) in [(20u16, 6u16), (40, 10), (80, 24)] {
            let mut b = buf(w, h);
            let inner = Rect::new(1, 1, w - 2, h - 2);
            let vals: Vec<Option<f64>> = (0..64).map(|i| Some(i as f64 * 1.5)).collect();
            heatmap(&mut b, inner, &t(), &vals, Pattern::Digital, 2);
            let up: Vec<f64> = (0..200).map(|i| (i % 17) as f64).collect();
            mirrored(
                &mut b,
                inner,
                &t(),
                &up,
                &up,
                Style::default(),
                Style::default(),
            );
            waffle(
                &mut b,
                inner,
                &[Slice {
                    label: "a".into(),
                    value: 1.0,
                    pattern: Pattern::Solid,
                    style: Style::default(),
                }],
                Style::default(),
            );
            for (i, line) in text(&b).lines().enumerate() {
                assert!(
                    format::width(line.trim_end()) <= w as usize,
                    "第 {i} 列超出 {w} 欄"
                );
                if i == 0 || i == (h - 1) as usize {
                    assert!(line.trim().is_empty(), "第 {i} 列在區域外卻被畫到");
                }
            }
        }
    }

    #[test]
    fn zero_sized_areas_are_a_no_op_not_a_panic() {
        let mut b = buf(10, 4);
        for r in [
            Rect::new(0, 0, 0, 4),
            Rect::new(0, 0, 10, 0),
            Rect::new(0, 0, 0, 0),
        ] {
            heatmap(&mut b, r, &t(), &[Some(1.0)], Pattern::Solid, 2);
            mirrored(
                &mut b,
                r,
                &t(),
                &[1.0],
                &[1.0],
                Style::default(),
                Style::default(),
            );
            waffle(&mut b, r, &[], Style::default());
            strip(&mut b, 0, 0, 0, &t(), Some(50.0));
        }
        assert!(text(&b).trim().is_empty());
    }

    #[test]
    fn heatmap_marks_unavailable_cores_instead_of_showing_them_as_idle() {
        let mut b = buf(20, 2);
        let vals = vec![Some(0.0), None, Some(100.0)];
        heatmap(
            &mut b,
            Rect::new(0, 0, 20, 2),
            &t(),
            &vals,
            Pattern::Digital,
            2,
        );
        let line = text(&b).lines().next().unwrap().to_owned();
        // 中間那格必須跟「0%」看起來不一樣
        let chars: Vec<char> = line.chars().collect();
        assert_ne!(chars[0], chars[2], "拿不到值的核心不能跟閒置的核心長一樣");
        assert_eq!(chars[2], '·');
    }

    #[test]
    fn solid_waffle_cells_leave_a_hairline_between_rows() {
        // 兩列實心格要分得開：實心用 ▇（上緣留一條縫），不是 █
        assert_eq!(waffle_glyph(Pattern::Solid), '▇');
        assert_eq!(waffle_glyph(Pattern::Shade), Pattern::Shade.glyph());
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        let slices = [Slice {
            label: "a".into(),
            value: 1.0,
            pattern: Pattern::Solid,
            style: Style::default(),
        }];
        waffle(&mut buf, area, &slices, Style::default());
        for x in 0..10 {
            assert_eq!(buf[(x, 0)].symbol(), "▇");
            assert_eq!(buf[(x, 1)].symbol(), "▇");
        }
    }

    #[test]
    fn waffle_never_lets_a_small_slice_vanish() {
        // 用量很小的人也要看得到自己 —— 至少一格
        let mut b = buf(20, 5);
        let slices: Vec<Slice> = [1000.0, 1.0, 1.0]
            .iter()
            .enumerate()
            .map(|(i, v)| Slice {
                label: format!("u{i}"),
                value: *v,
                pattern: crate::ui::visual::pattern::COMPOSITION_ORDER[i],
                style: Style::default().fg(Color::Reset),
            })
            .collect();
        waffle(&mut b, Rect::new(0, 0, 20, 5), &slices, Style::default());
        let joined: String = text(&b).lines().collect();
        for s in &slices {
            assert!(
                joined.contains(waffle_glyph(s.pattern)),
                "{} 完全沒被畫出來",
                s.label
            );
        }
    }

    #[test]
    fn waffle_with_no_data_draws_nothing_rather_than_dividing_by_zero() {
        let mut b = buf(10, 3);
        assert_eq!(
            waffle(&mut b, Rect::new(0, 0, 10, 3), &[], Style::default()),
            0
        );
        let slices = [Slice {
            label: "z".into(),
            value: 0.0,
            pattern: Pattern::Solid,
            style: Style::default(),
        }];
        assert_eq!(
            waffle(&mut b, Rect::new(0, 0, 10, 3), &slices, Style::default()),
            0
        );
        assert!(text(&b).trim().is_empty());
    }

    #[test]
    fn mirrored_puts_download_above_and_upload_below_the_midline() {
        let mut b = buf(8, 6);
        // 只有下載
        mirrored(
            &mut b,
            Rect::new(0, 0, 8, 6),
            &t(),
            &[0.0; 8],
            &[10.0; 8],
            Style::default(),
            Style::default(),
        );
        // lines() 會吃掉結尾的空行，補回來才對得上列號
        let mut lines: Vec<String> = text(&b).lines().map(|s| s.to_owned()).collect();
        lines.resize(6, String::new());
        let top_ink = lines[0..3].iter().filter(|l| !l.trim().is_empty()).count();
        let bottom_ink = lines[3..6]
            .iter()
            .filter(|l| !l.trim().is_empty() && l.contains('█'))
            .count();
        assert!(top_ink > 0, "下載應該畫在中線上方");
        assert_eq!(bottom_ink, 0, "沒有上傳時下半部不該有波形");
    }

    #[test]
    fn strip_shows_unavailable_differently_from_zero() {
        let mut a = buf(6, 1);
        let mut z = buf(6, 1);
        strip(&mut a, 0, 0, 6, &t(), None);
        strip(&mut z, 0, 0, 6, &t(), Some(0.0));
        assert_ne!(text(&a), text(&z), "n/a 不能跟 0% 長一樣");
    }

    #[test]
    fn chips_stop_instead_of_drawing_half_of_one() {
        let mut b = buf(12, 1);
        let items = vec![
            ("SSD".to_owned(), Style::default()),
            ("EXT4".to_owned(), Style::default()),
            ("VERYLONGONE".to_owned(), Style::default()),
        ];
        chips(&mut b, Rect::new(0, 0, 12, 1), &items);
        let line = text(&b).lines().next().unwrap().to_owned();
        assert!(format::width(&line) <= 12);
        // 括號一定成對，不能出現半個 chip
        assert_eq!(
            line.matches('[').count(),
            line.matches(']').count(),
            "畫出了半個 chip：{line:?}"
        );
    }
}
