//! Process 頁面：可排序、可篩選、可捲動的完整行程列表。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use crate::app::App;
use crate::collectors::process::SortKey;
use crate::metrics::model::Severity;
use crate::ui::format;
use crate::ui::layout::clamp_scroll;
use crate::ui::widgets::panel::{inset, Panel};

/// 欄位定義：(標題, 寬度, 靠右, 對應的排序鍵)
const COLUMNS: &[(&str, u16, bool, Option<SortKey>)] = &[
    ("PID", 7, true, Some(SortKey::Pid)),
    ("PPID", 7, true, None),
    ("USER", 12, false, Some(SortKey::User)),
    ("S", 1, false, None),
    ("CPU%", 6, true, Some(SortKey::Cpu)),
    ("MEM%", 6, true, None),
    ("RSS", 10, true, Some(SortKey::Memory)),
    ("VIRT", 10, true, None),
    ("THR", 5, true, Some(SortKey::Threads)),
    ("TIME", 9, true, None),
];

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let p = app.state.process.state();
    let mem_total = app.state.memory.state().total();

    let procs = p.top(app.sort, 0, &app.filter);
    let title = if app.filter.is_empty() {
        format!("Processes  (sort: {})", app.sort.label())
    } else {
        format!(
            "Processes  (sort: {}  filter: {})",
            app.sort.label(),
            app.filter
        )
    };
    let states: Vec<String> = {
        let mut v: Vec<_> = p.by_state.iter().collect();
        v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        v.iter().take(4).map(|(c, n)| format!("{c}{n}")).collect()
    };
    let inner = Panel::new(theme, &title)
        .subtitle(format!(
            "{}/{} 顯示 · {} 執行緒 · {}",
            procs.len(),
            p.total,
            p.threads,
            states.join(" ")
        ))
        .render(area, buf);
    let panel = app.regions.add_list_panel(area, "proc.cpu", "Processes");
    let inner = inset(inner, 1, 0);
    if inner.height < 2 {
        return;
    }

    // 表頭：目前排序的欄位加上底線與強調色
    let mut x = inner.x;
    for (name, w, right, key) in COLUMNS {
        if x + w > inner.x + inner.width {
            break;
        }
        let active = *key == Some(app.sort);
        let style = if active {
            theme
                .bold(theme.palette.accent)
                .add_modifier(Modifier::UNDERLINED)
        } else {
            theme.dim_style().add_modifier(Modifier::UNDERLINED)
        };
        let text = if *right {
            format::pad_left(name, *w as usize)
        } else {
            format::pad(name, *w as usize)
        };
        buf.set_string(x, inner.y, text, style);
        x += w + 1;
    }
    let cmd_x = x;
    if cmd_x < inner.x + inner.width {
        buf.set_string(
            cmd_x,
            inner.y,
            format::pad(
                "COMMAND",
                (inner.x + inner.width).saturating_sub(cmd_x) as usize,
            ),
            theme.dim_style().add_modifier(Modifier::UNDERLINED),
        );
    }

    let visible = inner.height.saturating_sub(1) as usize;
    let scroll = clamp_scroll(app.scroll, procs.len(), visible);
    // 回報上界，這樣在清單底部按 ↓ 不會變成看不見的狀態累加
    app.set_scroll_bound(procs.len().saturating_sub(visible));
    let me = crate::collectors::util::real_uid();

    for (i, proc) in procs.iter().skip(scroll).take(visible).enumerate() {
        let y = inner.y + 1 + i as u16;
        let mine = proc.uid == me;
        let fg = if mine {
            theme.style(theme.palette.fg)
        } else {
            theme.dim_style()
        };
        let mem_pct = proc.mem_percent(mem_total);
        let state_sev = proc.state_severity();

        let cells: [(String, Style); 10] = [
            (proc.pid.to_string(), theme.faint_style()),
            (proc.ppid.to_string(), theme.faint_style()),
            (
                proc.user.clone(),
                if mine {
                    theme.style(theme.palette.accent)
                } else {
                    theme.dim_style()
                },
            ),
            (proc.state.to_string(), theme.severity_style(state_sev)),
            (
                format!("{:.1}", proc.cpu_percent),
                theme.severity_style(Severity::from_percent(proc.cpu_percent.min(100.0))),
            ),
            (
                format!("{mem_pct:.1}"),
                theme.severity_style(Severity::from_percent(mem_pct)),
            ),
            (
                format::bytes(proc.rss as f64),
                theme.style(theme.palette.memory),
            ),
            (format::bytes(proc.vsize as f64), theme.faint_style()),
            (proc.threads.to_string(), theme.faint_style()),
            (format::duration(proc.cpu_time_secs), theme.faint_style()),
        ];
        // 斑馬紋：只換底色不換前景，所以不影響任何顏色語義。
        // 高密度表格裡它是唯一能讓視線橫向不跑掉的東西。
        if app.deco().at_least(crate::ui::visual::Decoration::Standard) && i % 2 == 1 {
            buf.set_style(
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
                Style::default().bg(theme.c(theme.palette.surface)),
            );
        }
        let mut x = inner.x;
        for (idx, (text, style)) in cells.iter().enumerate() {
            let (_, w, right, _) = COLUMNS[idx];
            if x + w > inner.x + inner.width {
                break;
            }
            let t = if right {
                format::pad_left(text, w as usize)
            } else {
                format::pad(text, w as usize)
            };
            buf.set_string(x, y, t, *style);
            x += w + 1;
        }
        if cmd_x < inner.x + inner.width {
            let w = (inner.x + inner.width).saturating_sub(cmd_x) as usize;
            buf.set_string(cmd_x, y, format::truncate(&proc.cmdline, w), fg);
        }

        // CPU 與 MEM 各補一條極短的強度條。
        //
        // 數字就在旁邊，這裡不是要讀出精確值，是要讓眼睛能在幾十列裡
        // 「掃」出哪幾列偏高 —— 那是純數字表格做不到的。
        // 只在寬度真的有餘裕時畫，資料欄永遠優先。
        if app.deco().at_least(crate::ui::visual::Decoration::Minimal)
            && cmd_x + 24 < inner.x + inner.width
        {
            let strip_w = 4u16;
            let cmd_end = inner.x + inner.width;
            let sx = cmd_end.saturating_sub(strip_w * 2 + 3);
            if sx > cmd_x + 12 {
                crate::ui::widgets::viz::strip(
                    buf,
                    sx,
                    y,
                    strip_w,
                    theme,
                    Some(proc.cpu_percent.min(100.0)),
                );
                crate::ui::widgets::viz::strip(
                    buf,
                    sx + strip_w + 1,
                    y,
                    strip_w,
                    theme,
                    Some(mem_pct),
                );
            }
        }
        // 每一列行程都是一個可選節點。未來的「停掉這個行程」之類的
        // 操作就掛在這一層 —— 使用者選到誰，就對誰動作。
        app.regions.add_entity_item(
            panel,
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
            crate::metrics::describe::EntityRef::Process {
                pid: proc.pid,
                starttime: proc.starttime,
            },
            format!("{} ({})", proc.name, proc.pid),
        );
    }

    // 捲動指示
    if procs.len() > visible {
        let text = format!(
            "{}–{} / {}",
            scroll + 1,
            (scroll + visible).min(procs.len()),
            procs.len()
        );
        let x = inner.x + inner.width.saturating_sub(format::width(&text) as u16);
        buf.set_string(x, inner.y + inner.height - 1, text, theme.faint_style());
    }
}
