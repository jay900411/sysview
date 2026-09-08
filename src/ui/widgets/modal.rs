//! Overlay 的共用外框與操作提示。
//!
//! # 為什麼要有這一層
//!
//! sysview 有六種 overlay（啟動畫面、說明、Explain、確認、訊息、彩蛋），
//! 原本各畫各的：Explain 的提示壓在下框線上、說明頁把「其他任意鍵關閉」
//! 塞進鍵位表的一列、訊息視窗**完全沒有提示**（使用者不知道怎麼關掉）。
//! 同一種互動在不同地方長得不一樣，看起來就像不同的功能。
//!
//! # 規則
//!
//! 外框、標題位置、內縮、提示列的位置與樣式集中在這裡。**提示的內容則由
//! 每個 overlay 自己宣告它支援哪些操作**（[`Cap`]），不是各自寫死字串。
//!
//! 這樣做的重點不是省字，是**提示不會說謊**：Explain 原本寫「↑↓ 捲動 ·
//! 任意鍵關閉」，但 ↑↓ 明明不會關閉它。能力宣告出來之後，會捲動的視窗
//! 自動說「其他任意鍵關閉」，不會捲動的才說「任意鍵關閉」。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::{Clear, Widget};

use crate::theme::Theme;
use crate::ui::format;
use crate::ui::widgets::panel::{inset, Panel};

/// 一個 overlay 真的支援的操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    /// 內容比視窗長，↑↓ 用來捲動。
    ///
    /// 只有**真的捲得動**時才宣告 —— 內容塞得下卻寫著「↑↓ 捲動」，
    /// 使用者會按了發現沒反應。
    Scroll,
    /// ←→ 選擇、Enter 確認。
    Choose,
    /// 遊戲操作。
    Play,
    /// 死掉之後：只有重來。
    Restart,
    /// 任意鍵關閉。跟 [`Cap::Scroll`] 併用時會自動說「其他任意鍵」。
    CloseAny,
    /// 只有 Esc 關閉（畫面上還有別的鍵有作用時用這個）。
    CloseEsc,
    /// 任意鍵開始（啟動畫面）。
    StartAny,
    /// 正在輸入名字（彩蛋上榜）：Enter 送出、Esc 不記錄。
    /// 這時所有鍵都是在填名字，其他提示一概不寫。
    Name,
}

/// 一個 overlay 的外框宣告。
pub struct Modal<'a> {
    pub title: &'a str,
    pub subtitle: Option<String>,
    /// 邊框顏色。`None` 用主色。
    pub border: Option<Color>,
    /// 這個 overlay 支援什麼。順序不重要，提示會照固定順序排。
    pub caps: &'a [Cap],
}

/// 把外框畫進 `win`，回傳可以放內容的區域（提示列已經扣掉）。
///
/// 呼叫端負責決定 `win` 多大 —— 每種 overlay 需要的空間不一樣，
/// 但**框怎麼長、提示放哪裡**由這裡決定。
pub fn frame(buf: &mut Buffer, theme: &Theme, win: Rect, m: &Modal<'_>) -> Rect {
    Clear.render(win, buf);
    blank_straddling(buf, win);
    // Clear 過的格子背景是終端機的預設色；白底的終端機上會變成白框
    // （見 `Theme::surface_style`）。先把整個視窗塗上自己的底色。
    buf.set_style(win, theme.surface_style());
    let border = m.border.unwrap_or(theme.palette.accent);
    let mut p = Panel::new(theme, m.title).border_style(theme.style(border));
    if let Some(s) = &m.subtitle {
        p = p.subtitle(s.clone());
    }
    let inner = inset(p.render(win, buf), 1, 0);
    if inner.width == 0 || inner.height == 0 {
        return inner;
    }
    let line = hint(m.caps);
    if line.is_empty() || inner.height < 3 {
        // 太矮就不畫提示 —— 內容比提示重要，而且畫了也擠掉內容
        return inner;
    }
    render_hint(buf, theme, inner, &line);
    Rect {
        height: inner.height - 2,
        ..inner
    }
}

/// 把跨在視窗**左緣**上的全形字清掉。
///
/// 底下那一頁在 `win.x - 1` 畫了一個全形字（例如「終端」），它的右半格被
/// 視窗的框線蓋掉；ratatui 的 `Buffer::diff` 會跳過 continuation cell，
/// 所以左半格那個字元在真實終端機上還留著，貼在框線外面（使用者回報過
/// 一個「終」字卡在邊上）。這裡把它換成空白。
///
/// 只換**符號、不重設樣式**：`Cell::reset()` 會把背景色也清掉，這一格就
/// 變成終端機自己的預設底色 —— 在深色主題、淺色終端機上是一格白的。
/// 右、上、下三邊沒有這個問題：右緣外的 continuation cell 本來就是空的，
/// 上下沒有半格的概念。
pub fn blank_straddling(buf: &mut Buffer, win: Rect) {
    if win.x == 0 {
        return;
    }
    for y in win.y..win.y + win.height {
        if let Some(c) = buf.cell_mut((win.x - 1, y)) {
            if format::width(c.symbol()) > 1 {
                c.set_symbol(" ");
            }
        }
    }
}

/// 只畫提示列（給自己排版的 overlay 用，例如啟動畫面）。
pub fn render_hint(buf: &mut Buffer, theme: &Theme, inner: Rect, line: &[(&str, &str)]) {
    if inner.height == 0 {
        return;
    }
    let y = inner.y + inner.height - 1;
    let total: usize = line
        .iter()
        .map(|(k, d)| format::width(k) + format::width(d) + 1)
        .sum::<usize>()
        + 3 * line.len().saturating_sub(1);
    if total > inner.width as usize {
        return;
    }
    let key = theme.bold(theme.palette.accent);
    let dim = theme.faint_style();
    let mut x = inner.x + inner.width - total as u16;
    for (i, (k, d)) in line.iter().enumerate() {
        if i > 0 {
            buf.set_string(x + 1, y, "·", dim);
            x += 3;
        }
        buf.set_string(x, y, k, key);
        x += format::width(k) as u16;
        buf.set_string(x, y, format!(" {d}"), dim);
        x += format::width(d) as u16 + 1;
    }
}

/// 由能力推導出提示。順序固定：先能做什麼，最後才是怎麼離開。
pub fn hint(caps: &[Cap]) -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&str, &str)> = Vec::new();
    let has = |c: Cap| caps.contains(&c);
    if has(Cap::Name) {
        out.push(("Enter", "送出"));
        out.push(("Esc", "不記錄"));
        return out;
    }
    if has(Cap::Scroll) {
        out.push(("↑↓", "捲動"));
    }
    if has(Cap::Choose) {
        out.push(("←→", "選擇"));
        out.push(("Enter", "確認"));
    }
    if has(Cap::Play) {
        out.push(("空白 / ↑", "跳"));
        out.push(("↓", "蹲"));
        out.push(("r", "重來"));
    }
    if has(Cap::Restart) {
        // 死掉之後跳躍鍵也能重來，但要過一小段緩衝 —— 提示只寫穩定有效的
        out.push(("r", "重來"));
    }
    if has(Cap::StartAny) {
        out.push(("任意鍵", "開始"));
    } else if has(Cap::CloseEsc) {
        out.push(("Esc", if has(Cap::Choose) { "取消" } else { "關閉" }));
    } else if has(Cap::CloseAny) {
        // 會捲動的視窗，↑↓ 是**不會**關閉它的 —— 提示必須說清楚，
        // 不然使用者按 ↓ 想關掉卻只是往下捲。
        out.push(if has(Cap::Scroll) {
            ("其他任意鍵", "關閉")
        } else {
            ("任意鍵", "關閉")
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(caps: &[Cap]) -> String {
        hint(caps)
            .iter()
            .map(|(k, d)| format!("{k} {d}"))
            .collect::<Vec<_>>()
            .join(" · ")
    }

    #[test]
    fn while_typing_a_name_the_only_hints_are_submit_and_skip() {
        // 輸入框開著時 r 不是重來、Esc 不是關閉 —— 提示不能寫那些
        assert_eq!(words(&[Cap::Name]), "Enter 送出 · Esc 不記錄");
    }

    #[test]
    fn a_scrollable_modal_never_claims_that_any_key_closes_it() {
        // ↑↓ 在會捲動的視窗裡不是關閉鍵。提示說「任意鍵關閉」就是在說謊。
        assert_eq!(
            words(&[Cap::Scroll, Cap::CloseAny]),
            "↑↓ 捲動 · 其他任意鍵 關閉"
        );
        assert_eq!(words(&[Cap::CloseAny]), "任意鍵 關閉");
    }

    #[test]
    fn the_escape_word_follows_what_escape_actually_does() {
        // 有選項的時候 Esc 是「取消」，沒有的時候是「關閉」
        assert!(words(&[Cap::Choose, Cap::CloseEsc]).ends_with("Esc 取消"));
        assert!(words(&[Cap::Play, Cap::CloseEsc]).ends_with("Esc 關閉"));
    }

    #[test]
    fn the_way_out_is_always_listed_last() {
        for caps in [
            vec![Cap::Scroll, Cap::CloseAny],
            vec![Cap::Choose, Cap::CloseEsc],
            vec![Cap::Play, Cap::CloseEsc],
        ] {
            let h = hint(&caps);
            let last = h.last().unwrap().1;
            assert!(
                last == "關閉" || last == "取消" || last == "開始",
                "最後一項應該是離開方式，實際是 {last}"
            );
        }
    }

    #[test]
    fn the_game_over_hint_does_not_promise_moves_you_cannot_make() {
        // 死掉之後不能跳也不能蹲，提示就不該寫「空白 跳 · ↓ 蹲」
        let over = words(&[Cap::Restart, Cap::CloseEsc]);
        assert!(!over.contains("跳") && !over.contains("蹲"), "{over}");
        assert!(
            over.contains("r 重來") && over.contains("Esc 關閉"),
            "{over}"
        );
    }

    #[test]
    fn every_modal_says_how_to_get_out() {
        // 訊息視窗原本什麼提示都沒有，使用者不知道怎麼關掉它
        for caps in [
            vec![Cap::CloseAny],
            vec![Cap::Scroll, Cap::CloseAny],
            vec![Cap::Choose, Cap::CloseEsc],
            vec![Cap::Play, Cap::CloseEsc],
            vec![Cap::StartAny],
        ] {
            assert!(!hint(&caps).is_empty(), "{caps:?} 沒有告訴使用者怎麼離開");
        }
    }

    #[test]
    fn the_hint_is_dropped_rather_than_wrapped_when_it_does_not_fit() {
        use crate::theme::ColorDepth;
        let theme = Theme::new("default", ColorDepth::TrueColor);
        let area = Rect::new(0, 0, 20, 6);
        let mut buf = Buffer::empty(area);
        render_hint(&mut buf, &theme, area, &hint(&[Cap::Play, Cap::CloseEsc]));
        let text = crate::ui::buffer_text(&buf);
        assert!(text.trim().is_empty(), "擠不下卻硬畫了：{text:?}");
    }

    #[test]
    fn a_wide_character_straddling_the_left_edge_is_wiped() {
        // 視窗左框線落在全形字的第二格：那個字的第一格必須一起清掉，
        // 不然終端機會把整個字畫出來、黏在框線外面。
        let area = Rect::new(0, 0, 40, 6);
        let mut buf = Buffer::empty(area);
        buf.set_string(
            0,
            2,
            "終端機太小終端機太小",
            ratatui::style::Style::default(),
        );
        // 「終」在第 4 格（0-based），佔 4、5；視窗從第 5 格開始
        let win = Rect::new(5, 1, 20, 4);
        use crate::theme::ColorDepth;
        let theme = Theme::new("default", ColorDepth::TrueColor);
        frame(
            &mut buf,
            &theme,
            win,
            &Modal {
                title: "",
                subtitle: None,
                border: None,
                caps: &[Cap::CloseAny],
            },
        );
        let left = buf.cell((4, 2)).unwrap().symbol().to_owned();
        assert_eq!(left, " ", "跨在左框線上的字還留著：{left:?}");
        // 沒跨到的字不能被誤傷
        assert_eq!(buf.cell((0, 2)).unwrap().symbol(), "終");
    }

    #[test]
    fn blanking_the_straddler_keeps_its_background() {
        // 清掉的那一格要留著頁面的底色。`reset()` 會連背景一起清成終端機
        // 的預設色 —— 深色主題、淺色終端機上就是一格白的（使用者回報）。
        use ratatui::style::{Color, Style};
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let bg = Style::default().bg(Color::Rgb(10, 20, 30));
        buf.set_style(area, bg);
        buf.set_string(3, 1, "終端", bg);
        blank_straddling(&mut buf, Rect::new(4, 0, 10, 3));
        let c = buf.cell((3, 1)).unwrap();
        assert_eq!(c.symbol(), " ");
        assert_eq!(c.bg, Color::Rgb(10, 20, 30), "底色被清掉了");
    }

    #[test]
    fn the_frame_leaves_room_for_the_hint() {
        use crate::theme::ColorDepth;
        let theme = Theme::new("default", ColorDepth::TrueColor);
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        let content = frame(
            &mut buf,
            &theme,
            area,
            &Modal {
                title: "測試",
                subtitle: None,
                border: None,
                caps: &[Cap::Scroll, Cap::CloseAny],
            },
        );
        assert!(
            content.height + 2 <= area.height - 2,
            "內容區沒有把提示列扣掉"
        );
        assert!(content.y > area.y);
    }
}
