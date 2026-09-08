//! CPU 頁面。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::app::App;
use crate::config;
use crate::theme::Subsystem;
use crate::ui::common::{focus_panel, gauge_row, history_graph, KeyValues};
use crate::ui::format;
use crate::ui::layout::{cols, split_top};
use crate::ui::widgets::braille::GraphStyle;
use crate::ui::widgets::panel::{inset, Panel};
use ratatui::widgets::Widget;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let c = app.state.cpu.state();
    let theme = &app.theme;
    let color = theme.subsystem(Subsystem::Cpu);

    let graph_h = (area.height / 3).clamp(6, 12);
    let (top, rest) = split_top(area, graph_h);

    // ── 總使用率圖 ──────────────────────────────────────────────────────
    let temp_text = match c.temp.get() {
        Some(t) => {
            let (v, u) = config::temperature(t, &app.config.temperature_unit);
            format!("{v:.0}{u}")
        }
        None => "溫度 n/a".into(),
    };
    let sub = format!(
        "{}  ·  {}  ·  {}",
        format::reading(&c.usage),
        format::reading(&c.freq_avg()),
        temp_text
    );
    let inner = Panel::new(theme, "CPU Utilization")
        .subtitle(sub)
        .render(top, buf);
    app.regions.add_panel(top, "cpu.usage", "CPU Utilization");
    history_graph(
        buf,
        inset(inner, 1, 0),
        theme,
        &c.history,
        color,
        Some(100.0),
        |v| format!("{v:.0}%"),
        GraphStyle::Area,
    );

    // ── 下半：每核 + 統計 ───────────────────────────────────────────────
    let halves = cols(rest, &[11, 9]);
    render_cores(app, halves[0], buf);
    let right = cols(halves[1], &[1]);
    let (stats_area, procs_area) = split_top(right[0], right[0].height / 2);
    render_stats(app, stats_area, buf);
    render_top_procs(app, procs_area, buf);
}

fn render_cores(app: &App, area: Rect, buf: &mut Buffer) {
    let c = app.state.cpu.state();
    let theme = &app.theme;
    let color = theme.subsystem(Subsystem::Cpu);
    let phys = c.physical.map(|p| format!("{p} 核 / ")).unwrap_or_default();
    let (inner, panel) = focus_panel(
        app,
        area,
        buf,
        "Per-Core",
        Some(format!("{phys}{} 執行緒", c.logical)),
        "cpu.freq",
    );
    let inner = inset(inner, 1, 0);
    if inner.width < 20 || inner.height == 0 {
        return;
    }
    // 核心多到逐條列不完時，改畫熱度圖。
    //
    // 24 個邏輯核心用逐條長條要 24 列，一頁就沒了；熱度圖把同樣的資訊
    // 壓成幾列，而且「哪幾顆在忙」變成空間分佈，一眼就看得到，
    // 那是逐條列給不了的。核心少的機器仍然用逐條列，那樣資訊更細。
    let rows_needed = c
        .cores
        .len()
        .div_ceil(if inner.width >= 64 { 2 } else { 1 });
    // 放不下就自動改用熱度圖；`v` 可以隨時手動切換（兩種都留著，
    // 因為它們回答不同的問題 —— 逐條列有每顆的頻率，熱度圖有空間分佈）
    let auto_heatmap = rows_needed > inner.height as usize && c.cores.len() >= 8;
    if app.cpu_heatmap || auto_heatmap {
        render_core_heatmap(app, inner, buf, panel);
        return;
    }
    // 寬的時候排兩欄
    let per_col = if inner.width >= 64 { 2 } else { 1 };
    let col_w = inner.width / per_col;
    let rows = inner.height as usize;

    for (i, core) in c.cores.iter().enumerate() {
        let (r, col) = (i % rows, i / rows);
        if col >= per_col as usize {
            break;
        }
        let y = inner.y + r as u16;
        let x = inner.x + col as u16 * col_w;
        let freq = format::reading(&core.freq_mhz);
        gauge_row(
            buf,
            Rect {
                x,
                y,
                width: col_w.saturating_sub(1),
                height: 1,
            },
            theme,
            &format!("{i:>2}"),
            &core.usage,
            core.usage.get(),
            &format!("{:>6}  {:>9}", format::reading(&core.usage), freq),
            Some(ratatui::style::Style::default().fg(color)),
            2,
            18,
        );
        // 每一顆邏輯核心都能單獨選取與解釋
        app.regions.add_entity_item(
            panel,
            Rect {
                x,
                y,
                width: col_w.saturating_sub(1),
                height: 1,
            },
            crate::metrics::describe::EntityRef::CpuCore(i),
            format!("CPU {i}"),
        );
    }
}

/// 每顆核心一格的熱度圖。
///
/// 強度同時用顏色與字元密度編碼，所以單色終端下也讀得出來。
/// 每一格仍然是可選的 —— 選到就能按 `e` 看那顆核心的完整說明。
fn render_core_heatmap(app: &App, area: Rect, buf: &mut Buffer, panel: usize) {
    let c = app.state.cpu.state();
    let theme = &app.theme;
    let n = c.cores.len();
    if n == 0 || area.width < 12 {
        return;
    }

    // 排成接近方形的網格，每列開頭標出起始核心編號。
    // 沒有列標的話這只是一排色塊，看得出「有幾顆在忙」卻說不出是哪幾顆。
    const CELL: u16 = 3; // 兩欄畫熱度、一欄留白，核心之間才不會黏在一起
    let gutter = 4u16;
    let usable = area.width.saturating_sub(gutter);
    let max_per_row = (usable / CELL).max(1) as usize;
    let rows_avail = area.height.saturating_sub(2).max(1) as usize;
    // 每列最多 16 顆：再多就得從左數到右才找得到某一顆，
    // 那正是熱度圖想避免的。放不下就往寬的方向讓，寧可一列長也不要截掉核心。
    let per_row = max_per_row.min(n).clamp(1, 16);
    let per_row = if n.div_ceil(per_row) > rows_avail {
        n.div_ceil(rows_avail).min(max_per_row).max(per_row)
    } else {
        per_row
    };
    let grid_rows = n.div_ceil(per_row).min(rows_avail);

    for r in 0..grid_rows {
        let y = area.y + r as u16;
        buf.set_string(
            area.x,
            y,
            format::pad_left(&(r * per_row).to_string(), gutter as usize - 1),
            theme.faint_style(),
        );
        for col in 0..per_row {
            let i = r * per_row + col;
            let Some(core) = c.cores.get(i) else { break };
            let x = area.x + gutter + col as u16 * CELL;
            if x + CELL > area.x + area.width {
                break;
            }
            let (ch, style) = match core.usage.get() {
                // 拿不到值就標成不可用，不要拿 0 假裝閒置
                None => ('·', theme.style(theme.palette.estimated)),
                Some(p) => (
                    app.pattern().at((p / 100.0).clamp(0.0, 1.0)),
                    theme.severity_style(crate::metrics::model::Severity::from_percent(p)),
                ),
            };
            let cell: String = std::iter::repeat_n(ch, CELL as usize - 1).collect();
            buf.set_string(x, y, cell, style);
            app.regions.add_entity_item(
                panel,
                Rect {
                    x,
                    y,
                    width: CELL,
                    height: 1,
                },
                crate::metrics::describe::EntityRef::CpuCore(i),
                format!("CPU {i}"),
            );
        }
    }

    // 圖例 + 一句摘要。沒有圖例，熱度圖只是一片花紋。
    let mut y = area.y + grid_rows as u16;
    if y < area.y + area.height {
        let p = app.pattern();
        let legend = format!(
            "{} idle   {} 40%   {} 70%   {} 90%+   · n/a",
            p.at(0.05),
            p.at(0.45),
            p.at(0.75),
            p.at(1.0)
        );
        buf.set_string(
            area.x,
            y,
            format::truncate(&legend, area.width as usize),
            theme.faint_style(),
        );
        y += 1;
    }
    if y < area.y + area.height {
        let busy = c
            .cores
            .iter()
            .filter(|c| c.usage.get().unwrap_or(0.0) >= 70.0)
            .count();
        let hottest = c
            .cores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.usage.or_zero().total_cmp(&b.1.usage.or_zero()))
            .map(|(i, core)| format!("最忙 CPU {i} ({:.0}%)", core.usage.or_zero()))
            .unwrap_or_default();
        let text = format!("{busy} / {n} 顆超過 70%   {hottest}   v 切回逐條列");
        buf.set_string(
            area.x,
            y,
            format::truncate(&text, area.width as usize),
            theme.dim_style(),
        );
    }
}

fn render_stats(app: &App, area: Rect, buf: &mut Buffer) {
    let c = app.state.cpu.state();
    let theme = &app.theme;
    let (inner, panel) = focus_panel(app, area, buf, "Core Statistics", None, "cpu.load");
    let inner = inset(inner, 1, 0);
    let cg = &app.state.cgroup;

    let mut kv = KeyValues::new(theme)
        .focusable(app, panel)
        .key_width(16)
        .row_of(
            "Model",
            format::truncate(&c.model, (inner.width as usize).saturating_sub(17)),
            "cpu.model",
        )
        .row_of("Logical CPUs", c.logical.to_string(), "cpu.logical")
        .reading("Avg Frequency", &c.freq_avg())
        .reading("Package Temp", &c.temp)
        .row_of(
            "Load 1/5/15",
            format!("{:.2}  {:.2}  {:.2}", c.load[0], c.load[1], c.load[2]),
            "cpu.load",
        )
        .row_of(
            "Load per core",
            format!(
                "{:.2}  (= load ÷ {} 核)",
                c.load_per_core(),
                c.logical.max(1)
            ),
            "cpu.load_per_core",
        )
        .reading("I/O Wait", &c.iowait)
        .reading("Steal", &c.steal)
        .reading("Context switches", &c.ctxt_rate)
        .reading("Interrupts", &c.intr_rate)
        .reading("New processes", &c.fork_rate)
        .row_of(
            "Running/Blocked",
            format!(
                "{} / {}",
                format::reading(&c.running),
                format::reading(&c.blocked)
            ),
            "cpu.running_blocked",
        )
        .row_of("Uptime", format::duration(c.uptime_secs), "sys.uptime");

    // 在容器裡就把 cgroup 配額也秀出來 —— 否則使用者會誤以為自己有整台機器
    if let Some(limit) = cg.cpu_limit_cores {
        kv = kv.styled_row_of(
            "cgroup CPU limit",
            format!("{limit:.2} 核（容器配額）"),
            theme.style(theme.palette.warning),
            "cgroup.cpu",
        );
    }
    kv.render(inner, buf);
}

fn render_top_procs(app: &App, area: Rect, buf: &mut Buffer) {
    use crate::collectors::process::SortKey;
    let theme = &app.theme;
    let (inner, _) = focus_panel(app, area, buf, "Top CPU Consumers", None, "proc.cpu");
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    let procs = app
        .state
        .process
        .state()
        .top(SortKey::Cpu, inner.height as usize, "");
    for (i, p) in procs.iter().enumerate() {
        let y = inner.y + i as u16;
        let sev = crate::metrics::model::Severity::from_percent(p.cpu_percent.min(100.0));
        buf.set_string(
            inner.x,
            y,
            format::pad_left(&format!("{:.1}%", p.cpu_percent), 7),
            theme.severity_style(sev),
        );
        buf.set_string(
            inner.x + 8,
            y,
            format::pad(&p.user, 10),
            theme.faint_style(),
        );
        let w = inner.width.saturating_sub(19) as usize;
        buf.set_string(
            inner.x + 19,
            y,
            format::truncate(&p.name, w),
            theme.style(theme.palette.fg),
        );
    }
}
