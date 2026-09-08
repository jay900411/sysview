//! Describe / Explain 面板 —— sysview 與其他監控工具最大的差異。
//!
//! 選到畫面上任何一格按 `e`，都能回答：
//!
//! ```text
//! 這是什麼？                What is this?
//! 這台機器上的它現在是什麼？ Current
//! 這個值從哪裡來？          Source
//! sysview 怎麼取得？        How sysview got it
//! 怎麼算出來的？            Formula
//! 有什麼要注意的？          Things to know
//! 我要自己查要打什麼？      Equivalent commands
//! 還該一起看什麼？          Related
//! ```
//!
//! **全部 deterministic**：資料來自編譯期的 Linux 知識庫加上 collector
//! 已經取好的執行期狀態，沒有任何文字生成。
//!
//! 不適用的區塊直接不畫 —— 不要出現 `Formula: n/a` 這種噪音。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::metrics::describe::{Certainty, DescribeContent};
use crate::theme::Theme;
use crate::ui::layout;
use crate::ui::widgets::panel::inset;

/// 把一份 [`DescribeContent`] 排版成可顯示的行。抽出來才能單獨測內容完整性。
pub fn compose<'a>(c: &'a DescribeContent, theme: &Theme) -> Vec<Line<'a>> {
    let head = theme.bold(theme.palette.accent);
    let body = theme.style(theme.palette.fg);
    let dim = theme.dim_style();
    let faint = theme.faint_style();
    let warn = theme.style(theme.palette.warning);
    let ok = theme.style(theme.palette.ok);
    let est = theme.style(theme.palette.estimated);

    let mut out: Vec<Line> = Vec::new();
    let section = |title: &'static str, lines: Vec<Line<'a>>, out: &mut Vec<Line<'a>>| {
        if lines.is_empty() {
            return;
        }
        if !out.is_empty() {
            out.push(Line::from(""));
        }
        out.push(Line::from(Span::styled(title, head)));
        out.extend(lines);
    };

    if !c.summary.is_empty() {
        section(
            "What is this?",
            vec![Line::from(Span::styled(c.summary.as_str(), body))],
            &mut out,
        );
    }

    // Current：這台機器上的實際值。不確定的值一定要標出來。
    if !c.current.is_empty() {
        let width = c
            .current
            .iter()
            .map(|f| crate::ui::format::width(&f.label))
            .max()
            .unwrap_or(0)
            .min(28);
        let rows: Vec<Line> = c
            .current
            .iter()
            .map(|f| {
                let value_style = match f.certainty {
                    Certainty::Exact => body,
                    Certainty::Derived => body,
                    Certainty::Estimated => est,
                    Certainty::Unavailable => faint,
                };
                let mut spans = vec![
                    Span::styled("  ", dim),
                    Span::styled(crate::ui::format::pad(&f.label, width), dim),
                    Span::styled("  ", dim),
                    Span::styled(f.value.as_str(), value_style),
                ];
                if let Some(tag) = f.certainty.tag() {
                    spans.push(Span::styled(format!("  ({tag})"), est));
                }
                Line::from(spans)
            })
            .collect();
        section("Current", rows, &mut out);
    }

    if let Some(d) = &c.derivation {
        section(
            "Formula",
            vec![Line::from(Span::styled(d.as_str(), body))],
            &mut out,
        );
    }
    section(
        "Source",
        c.source
            .iter()
            .map(|s| Line::from(vec![Span::styled("  ", dim), Span::styled(s.as_str(), dim)]))
            .collect(),
        &mut out,
    );
    if let Some(h) = &c.how_obtained {
        section(
            "How sysview got it",
            vec![Line::from(Span::styled(h.as_str(), dim))],
            &mut out,
        );
    }
    section(
        "Things to know",
        c.pitfalls
            .iter()
            .map(|p| {
                Line::from(vec![
                    Span::styled("  ! ", warn.add_modifier(Modifier::BOLD)),
                    Span::styled(p.as_str(), body),
                ])
            })
            .collect(),
        &mut out,
    );
    section(
        "Equivalent commands",
        c.commands
            .iter()
            .map(|cmd| {
                Line::from(vec![
                    Span::styled("  $ ", dim),
                    Span::styled(cmd.as_str(), ok),
                ])
            })
            .collect(),
        &mut out,
    );
    if !c.related.is_empty() {
        let spans: Vec<Span> = c
            .related
            .iter()
            .enumerate()
            .flat_map(|(i, r)| {
                let mut v = Vec::new();
                if i > 0 {
                    v.push(Span::styled("  ·  ", dim));
                }
                v.push(Span::styled(r.as_str(), theme.style(theme.palette.network)));
                v
            })
            .collect();
        section("Related", vec![Line::from(spans)], &mut out);
    }
    if let Some(n) = &c.permission_note {
        section(
            "Permission",
            vec![Line::from(Span::styled(n.as_str(), warn))],
            &mut out,
        );
    }
    out
}

/// 把排好的行依視窗寬度折成「一行 = 螢幕一列」。
///
/// 自己折而不是交給 `Wrap`，是為了能**精確**知道總共佔幾列 ——
/// 捲動的上界要靠它，算錯就會捲進空白或看不到最後一行。
/// ratatui 有 `line_count`，但那是 unstable API，不適合當成產品依賴。
///
/// 折行用**顯示寬度**而不是字元數：中日韓字元佔兩欄，用字元數會折錯位置。
/// 有空白就在空白處折（英文句子、指令），沒有空白就直接折（中文）。
fn wrap_to_width<'a>(lines: Vec<Line<'a>>, width: usize) -> Vec<Line<'a>> {
    if width == 0 {
        return lines;
    }
    let mut out: Vec<Line<'a>> = Vec::with_capacity(lines.len());
    for line in lines {
        if line
            .spans
            .iter()
            .map(|s| crate::ui::format::width(&s.content))
            .sum::<usize>()
            <= width
        {
            out.push(line);
            continue;
        }
        // 攤平成 (字, 樣式) 再貪婪切，切完重新黏回 span
        let chars: Vec<(char, ratatui::style::Style)> = line
            .spans
            .iter()
            .flat_map(|sp| sp.content.chars().map(move |c| (c, sp.style)))
            .collect();
        let cw = |c: char| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);

        let mut start = 0usize;
        while start < chars.len() {
            let mut w = 0usize;
            let mut i = start;
            let mut last_space = None;
            while i < chars.len() {
                let c = chars[i].0;
                if w + cw(c) > width && i > start {
                    break;
                }
                if c == ' ' {
                    last_space = Some(i);
                }
                w += cw(c);
                i += 1;
            }
            let mut end = i;
            // 斷在單字中間就退回上一個空白，但不要退成空行
            if end < chars.len() && chars[end].0 != ' ' {
                if let Some(sp) = last_space {
                    if sp > start {
                        end = sp;
                    }
                }
            }
            out.push(rebuild(&chars[start..end]));
            start = end;
            // 折行處的那一個空白吃掉，後面的縮排保留
            if start < chars.len() && chars[start].0 == ' ' {
                start += 1;
            }
        }
    }
    out
}

/// 把 (字, 樣式) 序列黏回連續同樣式的 span。
fn rebuild<'a>(chars: &[(char, ratatui::style::Style)]) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<ratatui::style::Style> = None;
    for &(c, st) in chars {
        if cur != Some(st) {
            if let Some(prev) = cur.take() {
                spans.push(Span::styled(std::mem::take(&mut buf), prev));
            }
            cur = Some(st);
        }
        buf.push(c);
    }
    if let Some(st) = cur {
        spans.push(Span::styled(buf, st));
    }
    Line::from(spans)
}

pub struct Explain<'a> {
    pub content: &'a DescribeContent,
    pub theme: &'a Theme,
    /// 內容太長時捲動到第幾行
    pub scroll: u16,
    /// 算好的捲動上界寫回這裡，下一次按鍵才夾得住。
    /// 文字要依這個視窗的寬度折過行才知道總共幾行，那只有畫的時候才知道。
    pub max_scroll_out: Option<&'a std::cell::Cell<u16>>,
    /// 使用者實際框住的那一格在畫面上叫什麼。
    ///
    /// 面板裡的一列（`Logical CPUs`、`Interrupts`、`MAC`）跟它背後的 metric
    /// 不一定同名。標題要寫**使用者點的那個東西**，否則框住 `Logical CPUs`
    /// 卻看到標題寫「CPU Utilization」，會讓人以為選錯了。
    pub selected: Option<&'a str>,
}

impl Widget for Explain<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        use crate::ui::widgets::modal::{self, Cap};
        let win = layout::centered(area, 92, 34);
        // 標題 = 使用者框住的那一格；副標 = 它背後是哪個 metric。
        let title = self
            .selected
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(self.content.title.as_str());
        let subtitle = if title == self.content.title {
            match self.content.metric_id {
                Some(id) => format!("{} · {id}", self.content.type_name),
                None => self.content.type_name.to_owned(),
            }
        } else {
            match self.content.metric_id {
                Some(id) => format!("{} · {} · {id}", self.content.type_name, self.content.title),
                None => format!("{} · {}", self.content.type_name, self.content.title),
            }
        };

        // 捲不捲得動要先折過行才知道 —— 而提示要照**實際**能力寫，
        // 所以先用「假設沒有提示列」的寬度折一次，決定能力，再開框。
        let probe = inset(layout::centered(area, 92, 34), 2, 1);
        let lines = wrap_to_width(
            compose(self.content, self.theme),
            probe.width.max(1) as usize,
        );
        let scrolls = lines.len() as u16 > probe.height.saturating_sub(1);
        let caps: &[Cap] = if scrolls {
            &[Cap::Scroll, Cap::CloseAny]
        } else {
            &[Cap::CloseAny]
        };
        let inner = modal::frame(
            buf,
            self.theme,
            win,
            &modal::Modal {
                title,
                subtitle: Some(subtitle),
                border: None,
                caps,
            },
        );
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        // 真正的內容區寬度可能跟 probe 差一點，重折一次才不會少算行數
        let lines = wrap_to_width(compose(self.content, self.theme), inner.width as usize);
        let total = lines.len().min(u16::MAX as usize) as u16;
        let max_scroll = total.saturating_sub(inner.height);
        if let Some(cell) = self.max_scroll_out {
            cell.set(max_scroll);
        }
        Paragraph::new(lines)
            .scroll((self.scroll.min(max_scroll), 0))
            .render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wrapping_uses_display_width_not_character_count() {
        use super::wrap_to_width;
        use ratatui::text::Line;
        // 中日韓字元佔兩欄。用字元數折的話 10 個中文字會被當成寬度 10，
        // 實際上是 20 欄，框線就會被撐破。
        let lines = vec![Line::from(
            "這是一段很長的中文說明文字需要折行處理".to_owned(),
        )];
        let wrapped = wrap_to_width(lines, 12);
        assert!(wrapped.len() > 1, "超過寬度就該折行");
        for l in &wrapped {
            let w: usize = l
                .spans
                .iter()
                .map(|s| crate::ui::format::width(&s.content))
                .sum();
            assert!(w <= 12, "折出來的行寬 {w} 超過 12 欄：{l:?}");
        }
    }

    #[test]
    fn wrapping_prefers_to_break_at_spaces() {
        use super::wrap_to_width;
        use ratatui::text::Line;
        let lines = vec![Line::from("sysview --explain cpu.model".to_owned())];
        let wrapped = wrap_to_width(lines, 18);
        let first: String = wrapped[0]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(first, "sysview --explain", "應該在空白處折，不要切在字中間");
    }

    #[test]
    fn wrapping_keeps_every_character() {
        use super::wrap_to_width;
        use ratatui::text::Line;
        let src = "混合 English 與中文的一行 with some words 還有更多內容";
        let wrapped = wrap_to_width(vec![Line::from(src.to_owned())], 14);
        let joined: String = wrapped
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        // 折行只會把空白換成換行，其他字一個都不能少
        let a: String = src.chars().filter(|c| !c.is_whitespace()).collect();
        let b: String = joined.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(a, b, "折行不能吃掉或重複任何字");
    }

    #[test]
    fn short_lines_are_left_alone() {
        use super::wrap_to_width;
        use ratatui::text::Line;
        let wrapped = wrap_to_width(vec![Line::from("short".to_owned())], 40);
        assert_eq!(wrapped.len(), 1);
    }

    use super::*;
    use crate::collectors::{Intervals, SystemState};
    use crate::metrics::describe::{describe, DescribeTarget, EntityRef, Field};
    use crate::metrics::knowledge::METRICS;
    use crate::theme::ColorDepth;

    fn theme() -> Theme {
        Theme::new("default", ColorDepth::TrueColor)
    }
    fn state() -> SystemState {
        let mut s = SystemState::with_options(60, Intervals::default(), true);
        s.sample_all(std::time::Instant::now());
        std::thread::sleep(std::time::Duration::from_millis(120));
        s.sample_all(std::time::Instant::now());
        s
    }
    fn text(c: &DescribeContent) -> String {
        compose(c, &theme())
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_metric_renders_all_expected_sections() {
        let st = state();
        for m in METRICS {
            let c = describe(&DescribeTarget::Metric(m.id), &st);
            let t = text(&c);
            for sec in [
                "What is this?",
                "Formula",
                "Source",
                "Things to know",
                "Equivalent commands",
            ] {
                assert!(t.contains(sec), "{} 的說明缺少 {sec}", m.id);
            }
        }
    }

    #[test]
    fn sections_that_do_not_apply_are_omitted_entirely() {
        // 不該出現 "Formula: n/a" 這種佔位噪音
        let c = DescribeContent {
            title: "x".into(),
            type_name: "T",
            summary: "s".into(),
            ..Default::default()
        };
        let t = text(&c);
        assert!(!t.contains("Formula"), "沒有算式就不該顯示 Formula 區塊");
        assert!(!t.contains("Current"), "沒有當前值就不該顯示 Current 區塊");
        assert!(!t.contains("n/a"));
        assert!(t.contains("What is this?"));
    }

    #[test]
    fn estimated_values_are_visibly_tagged() {
        let c = DescribeContent {
            title: "x".into(),
            type_name: "T",
            current: vec![
                Field::exact("精確", "42"),
                Field::estimated("推估", "43"),
                Field::unavailable("拿不到", "unsupported"),
            ],
            ..Default::default()
        };
        let t = text(&c);
        assert!(t.contains("(estimated)"), "估計值必須標出來：{t}");
        assert!(t.contains("(unavailable)"));
        // 精確值不該被加註記
        let exact_line = t.lines().find(|l| l.contains("精確")).unwrap();
        assert!(!exact_line.contains('('), "精確值不該有註記：{exact_line}");
    }

    #[test]
    fn entities_render_without_panicking_on_this_machine() {
        let st = state();
        let mut targets = vec![
            DescribeTarget::Entity(EntityRef::CpuCore(0)),
            DescribeTarget::Entity(EntityRef::User(crate::collectors::util::real_uid())),
            DescribeTarget::Entity(EntityRef::Path("/".into())),
        ];
        for d in &st.gpu.state().devices {
            targets.push(DescribeTarget::Entity(EntityRef::Gpu(d.id.clone())));
        }
        for m in &st.disk.state().mounts {
            targets.push(DescribeTarget::Entity(EntityRef::Mount(
                m.mount_point.clone(),
            )));
        }
        for d in &st.disk.state().devices {
            targets.push(DescribeTarget::Entity(EntityRef::Disk(d.name.clone())));
        }
        for i in &st.network.state().interfaces {
            targets.push(DescribeTarget::Entity(EntityRef::NetworkInterface(
                i.name.clone(),
            )));
        }
        if let Some(p) = st.process.state().processes.first() {
            targets.push(DescribeTarget::Entity(EntityRef::Process {
                pid: p.pid,
                starttime: p.starttime,
            }));
        }
        for t in targets {
            let c = describe(&t, &st);
            assert!(!c.title.is_empty(), "{t:?} 沒有標題");
            assert!(!c.type_name.is_empty());
            assert!(!c.summary.is_empty(), "{t:?} 沒有 What is this?");
            assert!(!c.current.is_empty(), "{t:?} 沒有任何當前值");
            assert!(!c.source.is_empty(), "{t:?} 沒有標示資料來源");
            assert!(!c.commands.is_empty(), "{t:?} 沒有給對應的原生指令");
        }
    }

    #[test]
    fn renders_into_small_terminals_without_overflow() {
        let st = state();
        let c = describe(&DescribeTarget::Metric("cpu.load"), &st);
        let t = theme();
        for (w, h) in [(50u16, 12u16), (80, 24), (200, 60)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
            Explain {
                content: &c,
                theme: &t,
                scroll: 0,
                selected: None,
                max_scroll_out: None,
            }
            .render(Rect::new(0, 0, w, h), &mut buf);
        }
    }

    #[test]
    fn cjk_content_does_not_break_the_panel_border() {
        let st = state();
        let c = describe(&DescribeTarget::Metric("cpu.iowait"), &st);
        let t = theme();
        let mut buf = Buffer::empty(Rect::new(0, 0, 110, 36));
        Explain {
            content: &c,
            theme: &t,
            scroll: 0,
            selected: None,
            max_scroll_out: None,
        }
        .render(Rect::new(0, 0, 110, 36), &mut buf);
        let win = layout::centered(Rect::new(0, 0, 110, 36), 92, 34);
        assert_eq!(buf.cell((win.x, win.y)).unwrap().symbol(), "╭");
        assert_eq!(
            buf.cell((win.x + win.width - 1, win.y)).unwrap().symbol(),
            "╮",
            "中文內容不可撐破右上角"
        );
    }
}
