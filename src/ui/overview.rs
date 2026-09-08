//! 總覽：一頁看完整台機器的狀態。
//!
//! 版面依終端機大小自動降級（見 [`crate::ui::layout::Density`]）。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::app::App;
use crate::collectors::process::SortKey;
use crate::metrics::model::Severity;
use crate::theme::Subsystem;
use crate::ui::common::{focus_panel, gauge_row, history_graph, placeholder};
use crate::ui::format;
use crate::ui::layout::{cols, rows, Density};
use crate::ui::widgets::braille::GraphStyle;
use crate::ui::widgets::panel::inset;

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    match Density::of(area) {
        Density::TooSmall => {
            placeholder(
                buf,
                area,
                &app.theme,
                &format!(
                    "終端機太小（目前 {}×{}，需要至少 50×12）",
                    area.width, area.height
                ),
            );
        }
        Density::Wide => {
            // Highlights 只佔最底下一列。它以前是右下角一整張卡片，
            // 為了三行字吃掉 56×23 的版面，還把 Processes 擠掉一半 ——
            // 那塊空間現在還給面板，吉祥物則改成常駐浮水印（見
            // `ui::visual::watermark`），一格資料都不佔。
            let (area, strip) = reserve_strip(area);
            let c = cols(area, &[1, 1]);
            let left = rows(c[0], &[5, 4, 4]);
            let right = rows(c[1], &[3, 4, 5]);
            // 註冊順序就是預設焦點的順序（`descend` 取 `roots().first()`），
            // 所以主面板照分頁的順序登記：CPU → Memory → GPU → Storage →
            // Network → Processes。方向鍵是**幾何**判斷，不看這個順序，
            // 所以左右欄交錯登記不影響 ←→ 走位。
            cpu_panel(app, left[0], buf);
            mem_panel(app, right[0], buf);
            gpu_panel(app, left[1], buf);
            disk_panel(app, right[1], buf);
            net_panel(app, left[2], buf);
            proc_panel(app, right[2], buf);
            // Highlights **一定最後**。它是次要摘要，進到頁面時第一個焦點
            // 不該是它 —— 使用者按 ↓ 想看的是 CPU，不是一行重點。
            if let Some(s) = strip {
                highlights_strip(app, s, buf);
            }
        }
        Density::Tall => {
            let (area, strip) = reserve_strip(area);
            let r = rows(area, &[5, 3, 4, 3, 3, 4]);
            cpu_panel(app, r[0], buf);
            mem_panel(app, r[1], buf);
            gpu_panel(app, r[2], buf);
            disk_panel(app, r[3], buf);
            net_panel(app, r[4], buf);
            proc_panel(app, r[5], buf);
            // 跟 Wide 一樣，最後才登記 —— 先前這裡是先畫的，於是
            // 96–132 欄的畫面上按 ↓ 第一個焦點就落在 Highlights。
            if let Some(s) = strip {
                highlights_strip(app, s, buf);
            }
        }
        Density::Compact => {
            let r = rows(area, &[4, 3, 3, 4]);
            cpu_panel(app, r[0], buf);
            mem_panel(app, r[1], buf);
            gpu_panel(app, r[2], buf);
            proc_panel(app, r[3], buf);
        }
    }
}

/// 從內容區底下留一列給 Highlights 條。
///
/// 高度不夠就整條不出現 —— 面板被壓扁比少一列重點嚴重得多。
fn reserve_strip(area: Rect) -> (Rect, Option<Rect>) {
    const MIN_PANELS_H: u16 = 20;
    if area.height < MIN_PANELS_H + 1 {
        return (area, None);
    }
    let (top, bottom) = crate::ui::layout::split_top(area, area.height - 1);
    (top, Some(bottom))
}

/// Highlights 條：一列，把幾條重點並排。
///
/// 規則跟以前完全一樣 —— 最耗 CPU 的行程、最滿的掛載點、最忙的介面，
/// 每一條都算得出是哪個數字來的。改的只有版面：從右下角一整張卡片
/// 變成底下一列。每一段仍然可以選、可以按 `e` 看下去。
///
/// 放不下的段落直接不畫，不截半個字 —— 「最耗 CPU  anydes…」那種
/// 讀起來像壞掉。
fn highlights_strip(app: &App, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    if area.height == 0 || area.width < 24 {
        return;
    }
    let items = highlights(app);
    if items.is_empty() {
        return;
    }
    let panel = app
        .regions
        // 登記的名字就是畫面上那個字 —— 焦點列顯示 HIGHLIGHTS 時
        // 使用者要能一眼對到是哪一列。
        .add_list_panel(area, "sys.highlights", "HIGHLIGHTS");

    // 版面：`▏HIGHLIGHTS   ▲ CPU syncsvc (1322) 99%   ·   ● DISK / 40%   · …`
    //
    // 左邊那個 `▏` 是分頁列上同一個記號，讓這一列讀起來是 sysview 的一個
    // 區段，而不是誰印出來的一行 debug 字串。每段前面的 `●◐▲■○` 是嚴重
    // 程度 —— 那是單色終端也讀得出來的通道，顏色只是加強。
    let faint = theme.faint_style();
    let lead = format!("{}HIGHLIGHTS", crate::ui::visual::motif::SECTION);
    let mut x = area.x;
    let end = area.x + area.width;
    buf.set_string(x, area.y, &lead, theme.bold(theme.palette.accent));
    x += format::width(&lead) as u16 + 3;

    // 段落之間留寬一點。擠在一起的話三段會讀成一長串，
    // 而它們其實是三件不相干的事。
    const GAP: u16 = 5;
    for (i, hl) in items.iter().enumerate() {
        // 分隔點畫在**前面**那一段後面，不是自己後面 —— 畫在後面的話，
        // 最後一段收尾會掛一個指向空無的「·」。
        let sep = if i == 0 { 0 } else { GAP };
        let head = format!("{} {} ", hl.severity.symbol(), hl.tag);
        let w = (format::width(&head) + format::width(&hl.text)) as u16;
        if x + sep + w > end {
            break;
        }
        if sep > 0 {
            buf.set_string(x + 2, area.y, "·", faint);
            x += sep;
        }
        let x0 = x;
        buf.set_string(
            x,
            area.y,
            hl.severity.symbol(),
            theme.severity_style(hl.severity),
        );
        x += format::width(hl.severity.symbol()) as u16 + 1;
        buf.set_string(
            x,
            area.y,
            hl.tag,
            theme.style(theme.subsystem(hl.subsystem)),
        );
        x += hl.tag.len() as u16 + 1;
        buf.set_string(x, area.y, &hl.text, hl.style);
        x += format::width(&hl.text) as u16;

        let rect = Rect {
            x: x0,
            y: area.y,
            width: x - x0,
            height: 1,
        };
        match &hl.entity {
            Some(e) => app
                .regions
                .add_entity_item(panel, rect, e.clone(), hl.label.clone()),
            None => app
                .regions
                .add_item(panel, rect, "sys.highlights", hl.label.clone()),
        };
    }
}

/// 總覽上的一條重點。
struct Highlight {
    /// 這一條屬於哪個子系統。條上用它當標籤，跟頁首的 chip 同一套字。
    tag: &'static str,
    subsystem: Subsystem,
    /// 嚴重程度。條上畫成 `●◐▲■○` —— 這是**單色也讀得出來**的通道，
    /// 顏色只是加強。
    severity: Severity,
    /// 畫在畫面上的字
    text: String,
    style: ratatui::style::Style,
    /// 焦點列與說明視窗上的名字
    label: String,
    /// 這一條在講哪個具體物件。有的話，按 `e` 就直接解釋那個東西。
    entity: Option<crate::metrics::describe::EntityRef>,
}

/// 幾條由**確定性規則**算出來的重點。
///
/// 不做推測、不下結論 —— 只是把「這台機器上現在最值得看一眼的東西」
/// 挑出來，並附上它的實際數字。每一條都能選、都能按 `e` 看下去。
fn highlights(app: &App) -> Vec<Highlight> {
    use crate::metrics::describe::EntityRef;
    let t = &app.theme;
    let mut out = Vec::new();

    let procs = app.state.process.state();
    if let Some(p) = procs
        .processes
        .iter()
        .max_by(|a, b| a.cpu_percent.total_cmp(&b.cpu_percent))
    {
        if p.cpu_percent >= 1.0 {
            out.push(Highlight {
                tag: "CPU",
                subsystem: Subsystem::Cpu,
                severity: Severity::from_percent(p.cpu_percent.min(100.0)),
                text: format!("{} ({}) {:.0}%", p.name, p.pid, p.cpu_percent),
                style: t.severity_style(Severity::from_percent(p.cpu_percent.min(100.0))),
                label: format!("{} ({})", p.name, p.pid),
                entity: Some(EntityRef::Process {
                    pid: p.pid,
                    starttime: p.starttime,
                }),
            });
        }
    }
    let disk = app.state.disk.state();
    if let Some(m) = disk
        .mounts
        .iter()
        .max_by(|a, b| a.usage.or_zero().total_cmp(&b.usage.or_zero()))
    {
        out.push(Highlight {
            tag: "DISK",
            subsystem: Subsystem::Disk,
            severity: m.severity(),
            text: format!("{} {:.0}%", m.mount_point, m.usage.or_zero()),
            style: t.severity_style(m.severity()),
            label: m.mount_point.clone(),
            entity: Some(EntityRef::Mount(m.mount_point.clone())),
        });
    }
    let net = app.state.network.state();
    if let Some(i) = net
        .interfaces
        .iter()
        .filter(|i| !i.virtual_iface)
        .max_by(|a, b| {
            (a.rx_rate.or_zero() + a.tx_rate.or_zero())
                .total_cmp(&(b.rx_rate.or_zero() + b.tx_rate.or_zero()))
        })
    {
        out.push(Highlight {
            tag: "NET",
            subsystem: Subsystem::Network,
            severity: Severity::Ok,
            text: format!(
                "{} ↓{} ↑{}",
                i.name,
                format::bytes(i.rx_rate.or_zero()),
                format::bytes(i.tx_rate.or_zero())
            ),
            style: t.dim_style(),
            label: i.name.clone(),
            entity: Some(EntityRef::NetworkInterface(i.name.clone())),
        });
    }
    // GPU 閒置但 VRAM 佔著：那通常代表有東西掛在上面沒放掉，值得知道
    for d in &app.state.gpu.state().devices {
        let (Some(u), Some(used), Some(total)) = (
            d.utilization.get(),
            d.memory_used.get(),
            d.memory_total.get(),
        ) else {
            continue;
        };
        if u < 5.0 && total > 0.0 && used / total >= 0.5 {
            out.push(Highlight {
                tag: "GPU",
                subsystem: Subsystem::Gpu,
                severity: Severity::Notice,
                text: format!("{} 閒置但 VRAM 佔了 {:.0}%", d.name, used / total * 100.0),
                style: t.style(t.palette.warning),
                label: d.name.clone(),
                entity: Some(EntityRef::Gpu(d.id.clone())),
            });
        }
    }
    out
}

fn cpu_panel(app: &App, area: Rect, buf: &mut Buffer) {
    let c = app.state.cpu.state();
    let theme = &app.theme;
    let color = theme.subsystem(Subsystem::Cpu);
    let (inner, _panel) = focus_panel(
        app,
        area,
        buf,
        "CPU",
        Some(format!("{} 執行緒", c.logical)),
        "cpu.usage",
    );
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }

    // 第一列：使用率 + 頻率 + 溫度 + load
    let mut bits = vec![format::reading(&c.freq_avg())];
    if let Some(t) = c.temp.get() {
        let (v, u) = crate::config::temperature(t, &app.config.temperature_unit);
        bits.push(format!("{v:.0}{u}"));
    }
    bits.push(format!("load {:.2}", c.load[0]));
    if c.iowait.get().is_some_and(|v| v > 1.0) {
        bits.push(format!("iowait {}", format::reading(&c.iowait)));
    }
    gauge_row(
        buf,
        Rect { height: 1, ..inner },
        theme,
        "",
        &c.usage,
        c.usage.get(),
        &format::reading(&c.usage),
        Some(ratatui::style::Style::default().fg(color)),
        0,
        8,
    );
    if inner.height > 1 {
        buf.set_string(
            inner.x,
            inner.y + 1,
            format::truncate(&bits.join("  ·  "), inner.width as usize),
            theme.dim_style(),
        );
    }
    // 剩下的空間畫歷史圖 + 每核長條
    if inner.height > 2 {
        let body = Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        };
        let core_rows = if app.config.show_per_core {
            core_grid_rows(c.logical, body.width)
        } else {
            0
        };
        let graph_h = body.height.saturating_sub(core_rows);
        if graph_h > 0 {
            history_graph(
                buf,
                Rect {
                    height: graph_h,
                    ..body
                },
                theme,
                &c.history,
                color,
                Some(100.0),
                |v| format!("{v:.0}"),
                GraphStyle::Area,
            );
        }
        if core_rows > 0 {
            core_grid(
                app,
                Rect {
                    y: body.y + graph_h,
                    height: core_rows,
                    ..body
                },
                buf,
            );
        }
    }
}

fn core_grid_rows(n: usize, width: u16) -> u16 {
    if n == 0 || width < 16 {
        return 0;
    }
    let per_row = (width / 8).max(1) as usize;
    n.div_ceil(per_row).min(4) as u16
}

fn core_grid(app: &App, area: Rect, buf: &mut Buffer) {
    let c = app.state.cpu.state();
    let theme = &app.theme;
    let per_row = (area.width / 8).max(1) as usize;
    let cell_w = area.width / per_row as u16;
    for (i, core) in c.cores.iter().enumerate() {
        let (r, col) = (i / per_row, i % per_row);
        if r as u16 >= area.height {
            break;
        }
        let x = area.x + col as u16 * cell_w;
        let y = area.y + r as u16;
        buf.set_string(x, y, format!("{i:>2}"), theme.faint_style());
        let bw = cell_w.saturating_sub(4);
        if bw > 0 {
            crate::ui::widgets::gauge::Gauge::new(theme, core.usage.or_zero()).render(
                Rect {
                    x: x + 3,
                    y,
                    width: bw,
                    height: 1,
                },
                buf,
            );
        }
    }
}

fn mem_panel(app: &App, area: Rect, buf: &mut Buffer) {
    let m = app.state.memory.state();
    let theme = &app.theme;
    let color = theme.subsystem(Subsystem::Memory);
    let (inner, _panel) = focus_panel(
        app,
        area,
        buf,
        "Memory",
        Some(format::bytes(m.total() as f64)),
        "mem.used",
    );
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    let used = m.used_percent();
    gauge_row(
        buf,
        Rect { height: 1, ..inner },
        theme,
        "RAM",
        &used,
        used.get(),
        &format!(
            "{} / {}",
            format::bytes(m.used() as f64),
            format::bytes(m.total() as f64)
        ),
        Some(ratatui::style::Style::default().fg(color)),
        4,
        22,
    );
    let mut y = inner.y + 1;
    if m.swap_total() > 0 && y < inner.y + inner.height {
        let sw = m.swap_percent();
        gauge_row(
            buf,
            Rect {
                y,
                height: 1,
                ..inner
            },
            theme,
            "SWP",
            &sw,
            sw.get(),
            &format!(
                "{} / {}",
                format::bytes(m.swap_used() as f64),
                format::bytes(m.swap_total() as f64)
            ),
            Some(ratatui::style::Style::default().fg(color)),
            4,
            22,
        );
        y += 1;
    }
    if y + 1 < inner.y + inner.height {
        let h = (inner.y + inner.height).saturating_sub(y + 1);
        history_graph(
            buf,
            Rect {
                y,
                height: h,
                ..inner
            },
            theme,
            &m.history,
            color,
            Some(100.0),
            |v| format!("{v:.0}"),
            GraphStyle::Area,
        );
        y += h;
    }
    if y < inner.y + inner.height {
        buf.set_string(
            inner.x,
            y,
            format::truncate(
                &format!(
                    "cache {}  ·  buffers {}  ·  available {}",
                    format::bytes(m.reclaimable_cache() as f64),
                    format::bytes(m.buffers() as f64),
                    format::bytes(m.available() as f64)
                ),
                inner.width as usize,
            ),
            theme.faint_style(),
        );
    }
}

fn gpu_panel(app: &App, area: Rect, buf: &mut Buffer) {
    let g = app.state.gpu.state();
    let theme = &app.theme;
    let count = g.devices.len();
    let (inner, _panel) = focus_panel(
        app,
        area,
        buf,
        "GPU",
        Some(if count == 0 {
            "none".into()
        } else {
            format!("{count} 張")
        }),
        "gpu.util",
    );
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    let mut y = inner.y;
    if let Some(err) = &g.nvidia_error {
        buf.set_string(
            inner.x,
            y,
            format::truncate(&format!("✗ NVIDIA: {err}"), inner.width as usize),
            theme.bold(theme.palette.critical),
        );
        y += 1;
        if y < inner.y + inner.height {
            buf.set_string(
                inner.x,
                y,
                "按 3 看完整診斷",
                theme.style(theme.palette.warning),
            );
            y += 1;
        }
    }
    for dev in &g.devices {
        if y + 1 >= inner.y + inner.height {
            break;
        }
        let mut bits = Vec::new();
        if let Some(t) = dev.temperature.get() {
            bits.push(format!("{t:.0}°C"));
        }
        if let Some(p) = dev.power.get() {
            bits.push(format!("{p:.0}W"));
        }
        let name = format::truncate(&dev.name, inner.width.saturating_sub(16) as usize);
        buf.set_string(
            inner.x,
            y,
            &name,
            theme.bold(theme.subsystem(Subsystem::Gpu)),
        );
        if !bits.is_empty() {
            let t = bits.join("  ");
            let x = inner.x + inner.width.saturating_sub(format::width(&t) as u16);
            buf.set_string(x, y, t, theme.dim_style());
        }
        y += 1;
        gauge_row(
            buf,
            Rect {
                y,
                height: 1,
                ..inner
            },
            theme,
            if dev.utilization.quality.is_estimated() {
                "util~"
            } else {
                "util"
            },
            &dev.utilization,
            dev.utilization.get(),
            &format::reading(&dev.utilization),
            None,
            6,
            8,
        );
        y += 1;
        if dev.memory_total.get().is_some() && y < inner.y + inner.height {
            gauge_row(
                buf,
                Rect {
                    y,
                    height: 1,
                    ..inner
                },
                theme,
                "vram",
                &dev.memory_used,
                dev.memory_percent(),
                &format!(
                    "{} / {}",
                    format::reading(&dev.memory_used),
                    format::reading(&dev.memory_total)
                ),
                None,
                6,
                20,
            );
            y += 1;
        }
    }
    if count == 0 && g.nvidia_error.is_none() {
        placeholder(buf, inner, theme, "找不到 GPU");
    }
}

fn disk_panel(app: &App, area: Rect, buf: &mut Buffer) {
    let d = app.state.disk.state();
    let theme = &app.theme;
    let (inner, _panel) = focus_panel(
        app,
        area,
        buf,
        "Storage",
        Some(format!(
            "R {}  W {}",
            format::reading(&d.total_read),
            format::reading(&d.total_write)
        )),
        "disk.usage",
    );
    let inner = inset(inner, 1, 0);
    for (i, m) in d.mounts.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        gauge_row(
            buf,
            Rect {
                y,
                height: 1,
                ..inner
            },
            theme,
            &format::truncate(&m.mount_point, 16),
            &m.usage,
            m.usage.get(),
            &format!("{} free", format::bytes(m.available as f64)),
            None,
            16,
            14,
        );
    }
}

fn net_panel(app: &App, area: Rect, buf: &mut Buffer) {
    let n = app.state.network.state();
    let theme = &app.theme;
    let (inner, _panel) = focus_panel(
        app,
        area,
        buf,
        "Network",
        Some(format!(
            "↓{}  ↑{}",
            format::reading(&n.total_rx),
            format::reading(&n.total_tx)
        )),
        "net.throughput",
    );
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    let mut active: Vec<_> = n
        .interfaces
        .iter()
        .filter(|i| !i.virtual_iface && i.up)
        .collect();
    active.sort_by(|a, b| {
        (b.rx_rate.or_zero() + b.tx_rate.or_zero())
            .total_cmp(&(a.rx_rate.or_zero() + a.tx_rate.or_zero()))
    });
    let mut y = inner.y;
    for it in active.iter().take(2) {
        if y >= inner.y + inner.height {
            break;
        }
        buf.set_string(
            inner.x,
            y,
            format::pad(&it.name, 10),
            theme.bold(theme.subsystem(Subsystem::Network)),
        );
        buf.set_string(
            inner.x + 11,
            y,
            format!("↓{}", format::pad_left(&format::reading(&it.rx_rate), 11)),
            theme.style(theme.palette.ok),
        );
        buf.set_string(
            inner.x + 25,
            y,
            format!("↑{}", format::pad_left(&format::reading(&it.tx_rate), 11)),
            theme.style(theme.palette.warning),
        );
        y += 1;
    }
    if y < inner.y + inner.height {
        let h = (inner.y + inner.height).saturating_sub(y);
        history_graph(
            buf,
            Rect {
                y,
                height: h,
                ..inner
            },
            theme,
            &n.rx_history,
            theme.subsystem(Subsystem::Network),
            None,
            format::bytes,
            GraphStyle::Line,
        );
    }
}

fn proc_panel(app: &App, area: Rect, buf: &mut Buffer) {
    let p = app.state.process.state();
    let theme = &app.theme;
    let mem_total = app.state.memory.state().total();
    let (inner, _panel) = focus_panel(
        app,
        area,
        buf,
        "Processes",
        Some(format!("{} 個 · {} 執行緒", p.total, p.threads)),
        "proc.cpu",
    );
    let inner = inset(inner, 1, 0);
    if inner.height < 2 {
        return;
    }
    crate::ui::common::table_header(
        buf,
        inner,
        theme,
        &[
            ("PID", 7, true),
            ("USER", 10, false),
            ("CPU%", 6, true),
            ("RSS", 9, true),
        ],
    );
    let n = inner.height.saturating_sub(1) as usize;
    for (i, proc) in p.top(SortKey::Cpu, n, "").iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        let sev = Severity::from_percent(proc.cpu_percent.min(100.0));
        crate::ui::common::table_row(
            buf,
            Rect {
                y,
                height: 1,
                ..inner
            },
            &[
                (proc.pid.to_string(), 7, true, theme.faint_style()),
                (proc.user.clone(), 10, false, theme.dim_style()),
                (
                    format!("{:.1}", proc.cpu_percent),
                    6,
                    true,
                    theme.severity_style(sev),
                ),
                (
                    format::bytes(proc.rss as f64),
                    9,
                    true,
                    theme.style(theme.subsystem(Subsystem::Memory)),
                ),
            ],
        );
        let x = inner.x + 36;
        if x < inner.x + inner.width {
            let w = (inner.x + inner.width).saturating_sub(x) as usize;
            let _ = mem_total;
            buf.set_string(
                x,
                y,
                format::truncate(&proc.name, w),
                theme.style(theme.palette.fg),
            );
        }
    }
}
