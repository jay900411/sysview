//! Memory 頁面。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::app::App;
use crate::collectors::process::SortKey;
use crate::theme::Subsystem;
use crate::ui::common::history_graph;
use crate::ui::format;
use crate::ui::layout::{cols, split_top};
use crate::ui::widgets::braille::GraphStyle;
use crate::ui::widgets::panel::{inset, Panel};

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let m = app.state.memory.state();
    let theme = &app.theme;
    let color = theme.subsystem(Subsystem::Memory);

    let graph_h = (area.height / 3).clamp(6, 12);
    let (top, rest) = split_top(area, graph_h);
    let halves = cols(top, &[1, 1]);

    let inner = Panel::new(theme, "RAM Usage")
        .subtitle(format!(
            "{} / {}",
            format::bytes(m.used() as f64),
            format::bytes(m.total() as f64)
        ))
        .render(halves[0], buf);
    app.regions.add_panel(halves[0], "mem.used", "RAM Usage");
    history_graph(
        buf,
        inset(inner, 1, 0),
        theme,
        &m.history,
        color,
        Some(100.0),
        |v| format!("{v:.0}%"),
        GraphStyle::Area,
    );

    let swap_sub = if m.swap_total() == 0 {
        "no swap configured".to_owned()
    } else {
        format!(
            "{} / {}",
            format::bytes(m.swap_used() as f64),
            format::bytes(m.swap_total() as f64)
        )
    };
    let inner = Panel::new(theme, "Swap Usage")
        .subtitle(swap_sub)
        .render(halves[1], buf);
    app.regions.add_panel(halves[1], "mem.swap", "Swap Usage");
    history_graph(
        buf,
        inset(inner, 1, 0),
        theme,
        &m.swap_history,
        color,
        Some(100.0),
        |v| format!("{v:.0}%"),
        GraphStyle::Area,
    );

    // ── 組成長條 ────────────────────────────────────────────────────────
    // 6 列：框線 2 + 長條 1 + 空行 1 + 圖例 1 + 判讀 1。
    // 空行不是留白好看 —— 長條本身是一整排實心方塊，圖例緊貼著它時
    // 兩者會黏成一塊，看起來像長條有兩層。
    let comp_h = if rest.height >= 14 { 6 } else { 5 };
    let (comp, rest) = split_top(rest, comp_h);
    render_composition(app, comp, buf);

    let halves = cols(rest, &[11, 9]);
    render_detail(app, halves[0], buf);
    render_top_procs(app, halves[1], buf);
}

/// 一條把 used / buffers / cache / free 依比例畫出來的長條。
///
/// 這是 memory 頁最重要的教學元件：讓使用者直觀看到
/// 「cache 很大不是記憶體被吃光，那是可以回收的」。
fn render_composition(app: &App, area: Rect, buf: &mut Buffer) {
    let m = app.state.memory.state();
    let theme = &app.theme;
    let inner = Panel::new(theme, "Physical Memory Composition").render(area, buf);
    app.regions
        .add_panel(area, "mem.cached", "Physical Memory Composition");
    let inner = inset(inner, 1, 0);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    use crate::ui::visual::pattern::{self, Pattern, Segment};

    // 每一段配一個**紋理**，不是只有顏色。
    //
    // 紋理的輕重也是語意：實心 = 已經用掉拿不回來，網點 = 可以回收，
    // 點 = 空的。單色終端、8 色 SSH、色盲讀者都還讀得出來這條長條，
    // 而顏色一旦失效，純色長條就只剩一整條同樣的方塊。
    let segments = [
        Segment {
            label: "used".into(),
            value: m.used() as f64,
            pattern: Pattern::Solid,
            color: theme.c(theme.severity_color(crate::metrics::model::Severity::Critical)),
        },
        Segment {
            label: "buffers".into(),
            value: m.buffers() as f64,
            pattern: Pattern::Shade,
            color: theme.c(theme.palette.warning),
        },
        Segment {
            label: "cache".into(),
            value: m.reclaimable_cache() as f64,
            pattern: Pattern::Braille,
            color: theme.c(theme.palette.network),
        },
        Segment {
            label: "free".into(),
            value: m.free() as f64,
            pattern: Pattern::Dot,
            color: theme.c(theme.palette.ok),
        },
    ];

    let cells = pattern::compose(
        &segments,
        inner.width as usize,
        '·',
        theme.c(theme.palette.faint),
    );
    for (i, (ch, color)) in cells.iter().enumerate() {
        buf.set_string(
            inner.x + i as u16,
            inner.y,
            ch.to_string(),
            Style::default().fg(*color),
        );
    }

    // 圖例：符號用該段自己的紋理，這樣圖例與長條是同一套語言。
    //
    // 中間空一列。長條本身就是一整排實心方塊，圖例緊貼在下面時
    // 兩者會黏成一塊，看起來像長條有兩層。
    let legend_y = inner.y + 2;
    if legend_y < inner.y + inner.height {
        let mut lx = inner.x;
        for seg in &segments {
            let label = format!("{} {}", seg.label, format::bytes(seg.value));
            let w = format::width(&label) as u16 + 4;
            if lx + w > inner.x + inner.width {
                break;
            }
            buf.set_string(
                lx,
                legend_y,
                seg.pattern.glyph().to_string(),
                Style::default().fg(seg.color),
            );
            buf.set_string(lx + 2, legend_y, &label, theme.dim_style());
            lx += w;
        }
    }

    // 一句話的判讀。規則是寫死的，不是猜的。
    if inner.height >= 5 {
        let used_pct = m.used_percent().get().unwrap_or(0.0);
        let cache_pct = m.reclaimable_cache() as f64 / m.total().max(1) as f64 * 100.0;
        let swap_active = m.swap_used() > 0;
        let (text, style) = if used_pct >= 90.0 {
            (
                "記憶體壓力高：可回收的部分已經不多了。",
                theme.style(theme.palette.critical),
            )
        } else if swap_active {
            (
                "有東西被換到 swap —— 曾經不夠用過。",
                theme.style(theme.palette.warning),
            )
        } else if cache_pct >= 40.0 {
            (
                "大量 page cache。那是好事，需要時會自動讓出來。",
                theme.style(theme.palette.ok),
            )
        } else {
            ("壓力低。", theme.dim_style())
        };
        buf.set_string(
            inner.x,
            inner.y + 4,
            format::truncate(text, inner.width as usize),
            style,
        );
    }
}

fn render_detail(app: &App, area: Rect, buf: &mut Buffer) {
    let m = app.state.memory.state();
    let theme = &app.theme;
    let inner = Panel::new(theme, "Detail (/proc/meminfo)").render(area, buf);
    app.regions
        .add_panel(area, "mem.available", "Detail (/proc/meminfo)");
    let inner = inset(inner, 1, 0);

    const KEYS: &[&str] = &[
        "MemTotal",
        "MemFree",
        "MemAvailable",
        "Buffers",
        "Cached",
        "SwapCached",
        "Active",
        "Inactive",
        "Dirty",
        "Writeback",
        "AnonPages",
        "Mapped",
        "Shmem",
        "Slab",
        "SReclaimable",
        "KernelStack",
        "PageTables",
        "CommitLimit",
        "Committed_AS",
    ];
    let rows = inner.height as usize;
    let per_col = if inner.width >= 56 { 2 } else { 1 };
    let col_w = inner.width / per_col;

    for (i, key) in KEYS.iter().enumerate() {
        let (r, c) = (i % rows.max(1), i / rows.max(1));
        if c >= per_col as usize {
            break;
        }
        let y = inner.y + r as u16;
        let x = inner.x + c as u16 * col_w;
        let val = match m.get(key) {
            Some(v) => format::bytes(v as f64),
            None => "—".into(),
        };
        buf.set_string(x, y, format::pad(key, 17), theme.dim_style());
        buf.set_string(
            x + 17,
            y,
            format::pad_left(&val, (col_w.saturating_sub(18)) as usize),
            theme.style(theme.palette.fg),
        );
    }

    // cgroup 限制
    if let (Some(limit), Some(cur)) = (
        app.state.cgroup.memory_limit,
        app.state.cgroup.memory_current,
    ) {
        let y = inner.y + inner.height.saturating_sub(1);
        buf.set_string(
            inner.x,
            y,
            format::truncate(
                &format!(
                    "cgroup 限制 {} / 目前 {}（容器內看到的 MemTotal 是宿主機的）",
                    format::bytes(limit as f64),
                    format::bytes(cur as f64)
                ),
                inner.width as usize,
            ),
            theme.style(theme.palette.warning),
        );
    }
}

fn render_top_procs(app: &App, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let total = app.state.memory.state().total();
    let inner = Panel::new(theme, "Top Memory Consumers")
        .subtitle("RSS 會重複計算共享函式庫")
        .render(area, buf);
    app.regions
        .add_panel(area, "proc.rss", "Top Memory Consumers");
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    let procs = app
        .state
        .process
        .state()
        .top(SortKey::Memory, inner.height as usize, "");
    for (i, p) in procs.iter().enumerate() {
        let y = inner.y + i as u16;
        let pct = p.mem_percent(total);
        buf.set_string(
            inner.x,
            y,
            format::pad_left(&format::bytes(p.rss as f64), 10),
            theme.style(theme.subsystem(Subsystem::Memory)),
        );
        buf.set_string(
            inner.x + 11,
            y,
            format::pad_left(&format!("{pct:.1}%"), 6),
            theme.severity_style(crate::metrics::model::Severity::from_percent(pct)),
        );
        buf.set_string(
            inner.x + 18,
            y,
            format::pad(&p.user, 10),
            theme.faint_style(),
        );
        let w = inner.width.saturating_sub(29) as usize;
        buf.set_string(
            inner.x + 29,
            y,
            format::truncate(&p.name, w),
            theme.style(theme.palette.fg),
        );
    }
}
