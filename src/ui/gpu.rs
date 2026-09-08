//! GPU 頁面。含 NVIDIA 驅動故障時的診斷。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::app::App;
use crate::collectors::gpu::{DiagLevel, GpuDevice};
use crate::theme::Subsystem;
use crate::ui::common::{gauge_row, history_graph, placeholder, KeyValues};
use crate::ui::format;
use crate::ui::layout::{cols, split_top};
use crate::ui::widgets::braille::GraphStyle;
use crate::ui::widgets::panel::{inset, Panel};

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let g = app.state.gpu.state();
    let theme = &app.theme;
    let mut rest = area;

    // 驅動掛掉時，診斷擺最上面 —— 那比任何數字都重要
    if let Some(err) = &g.nvidia_error {
        let h = ((g.diagnostics.len() + 4) as u16)
            .min(rest.height / 2)
            .max(5);
        let (top, r) = split_top(rest, h);
        render_diagnostics(app, top, buf, err);
        rest = r;
    }

    if g.devices.is_empty() {
        placeholder(buf, rest, theme, "找不到任何 GPU 裝置");
        return;
    }

    let per = (rest.height / g.devices.len() as u16).max(7);
    let mut y = rest.y;
    for dev in &g.devices {
        if y >= rest.y + rest.height {
            break;
        }
        let h = per.min((rest.y + rest.height).saturating_sub(y)).min(11);
        if h < 5 {
            break;
        }
        render_device(
            app,
            Rect {
                y,
                height: h,
                ..rest
            },
            buf,
            dev,
        );
        y += h;
    }
}

fn render_device(app: &App, area: Rect, buf: &mut Buffer, dev: &GpuDevice) {
    let theme = &app.theme;
    let g = app.state.gpu.state();
    let color = theme.subsystem(Subsystem::Gpu);

    let title = format!("{} · {}", dev.vendor.label(), dev.name);
    let mut sub = format!("[{}]", dev.backend);
    if let Some(d) = &dev.driver_version {
        sub = format!("driver {d}  {sub}");
    }
    // 讀值是估計的卡（Intel iGPU 的 RC6 反推）要一眼看得出來，
    // 不然使用者會把估計值當成量測值
    if dev.utilization.quality.is_estimated() {
        sub.push_str(" [ESTIMATED]");
    }
    // 活躍的卡用強調色邊框；閒置或估計的卡淡化但仍然清楚
    let active = dev.utilization.get().unwrap_or(0.0) >= 5.0;
    let border = if active {
        theme.style(theme.subsystem(Subsystem::Gpu))
    } else if dev.utilization.quality.is_estimated() {
        theme.style(theme.palette.estimated)
    } else {
        theme.border_style()
    };
    let inner = Panel::new(theme, &title)
        .subtitle(sub)
        .border_style(border)
        .render(area, buf);
    let panel = app.regions.add_entity_panel(
        area,
        crate::metrics::describe::EntityRef::Gpu(dev.id.clone()),
        title.as_str(),
    );
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }

    let halves = cols(inner, &[11, 9]);
    // 右欄往內縮兩格，避免撞到左邊圖表的座標軸
    let right = Rect {
        x: halves[1].x + 2,
        width: halves[1].width.saturating_sub(2),
        ..halves[1]
    };
    // 左：使用率圖 + gauge
    let graph_col = Rect {
        width: halves[0].width.saturating_sub(2),
        ..halves[0]
    };
    let (graph, bar) = split_top(graph_col, graph_col.height.saturating_sub(2));
    if let Some(series) = g.history.get(&dev.id) {
        history_graph(
            buf,
            graph,
            theme,
            series,
            color,
            Some(100.0),
            |v| format!("{v:.0}%"),
            GraphStyle::Area,
        );
    }
    if bar.height > 0 {
        gauge_row(
            buf,
            Rect { height: 1, ..bar },
            theme,
            "Util",
            &dev.utilization,
            dev.utilization.get(),
            &format::reading(&dev.utilization),
            None,
            6,
            10,
        );
    }
    // 沒有獨立 VRAM 介面的卡（Intel 內顯與系統共用記憶體）不畫這條長條。
    // 畫一條寫滿 "unsupported" 的長條只會讓人以為是壞掉了。
    if bar.height > 1 && dev.memory_total.get().is_some() {
        gauge_row(
            buf,
            Rect {
                y: bar.y + 1,
                height: 1,
                ..bar
            },
            theme,
            "VRAM",
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
    } else if bar.height > 1 {
        buf.set_string(
            bar.x,
            bar.y + 1,
            format::truncate(
                "VRAM    內顯與系統共用記憶體，DRM 沒有獨立的 VRAM 介面",
                bar.width as usize,
            ),
            theme.faint_style(),
        );
    }

    // 右：詳細數據。
    // 只列出這張卡**真的提供**的欄位 —— 一整排 "unsupported" 只是噪音，
    // 而且會蓋掉真正有值的資訊。哪些欄位不支援，Explain 裡會說明。
    let mut kv = KeyValues::new(theme).focusable(app, panel).key_width(15);
    if dev.temperature.get().is_some() {
        kv = kv.reading("Temperature", &dev.temperature);
    }
    match (dev.power.get(), dev.power_limit.get()) {
        (Some(p), Some(l)) => kv = kv.row_of("Power", format!("{p:.1} / {l:.0} W"), "gpu.power"),
        (Some(p), None) => kv = kv.row_of("Power", format!("{p:.1} W"), "gpu.power"),
        _ => {}
    }
    if dev.sm_clock.get().is_some() {
        kv = kv.reading("Core clock", &dev.sm_clock);
    }
    if let Some(r) = &dev.clock_range {
        kv = kv.row_of("Clock range", r.clone(), "gpu.clock");
    }
    if dev.mem_clock.get().is_some() {
        kv = kv.reading("Memory clock", &dev.mem_clock);
    }
    if dev.fan.get().is_some() {
        kv = kv.reading("Fan", &dev.fan);
    }
    if let Some(p) = &dev.pstate {
        kv = kv.row_of("Perf state", p.clone(), "gpu.pstate");
    }
    if let (Some(gen), Some(w)) = (dev.pcie_gen, dev.pcie_width) {
        kv = kv.row_of("PCIe link", format!("gen{gen} x{w}"), "gpu.pcie");
    }
    if dev.utilization.quality.is_estimated() {
        kv = kv.styled_row_of(
            "Data quality",
            "estimated · 按 e 看算法",
            theme.style(theme.palette.estimated),
            "gpu.intel_util",
        );
    }
    let procs = if dev.processes.is_empty() {
        "none".to_owned()
    } else if dev.process_list_complete {
        format!("{} 個", dev.processes.len())
    } else {
        format!("{} 個（僅自己的）", dev.processes.len())
    };
    kv = kv.row_of("Compute procs", procs, "gpu.process");
    kv.render(right, buf);
}

fn render_diagnostics(app: &App, area: Rect, buf: &mut Buffer, err: &str) {
    let theme = &app.theme;
    let crit = theme.style(theme.palette.critical);
    let inner = Panel::new(theme, "NVIDIA Diagnostics")
        .subtitle("NVML 目前不可用")
        .border_style(crit)
        .title_style(theme.bold(theme.palette.critical))
        .render(area, buf);
    app.regions
        .add_panel(area, "gpu.util", "NVIDIA Diagnostics");
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    buf.set_string(
        inner.x,
        inner.y,
        format::truncate(&format!("✗ {err}"), inner.width as usize),
        theme.bold(theme.palette.critical),
    );

    let mut y = inner.y + 1;
    for line in &app.state.gpu.state().diagnostics {
        if y >= inner.y + inner.height {
            break;
        }
        let style = match line.level {
            DiagLevel::Ok => theme.style(theme.palette.ok),
            DiagLevel::Warn => theme.style(theme.palette.warning),
            DiagLevel::Bad => theme.style(theme.palette.critical),
            DiagLevel::Info => theme.dim_style(),
        };
        buf.set_string(inner.x, y, line.level.symbol(), style);
        buf.set_string(
            inner.x + 2,
            y,
            format::truncate(&line.text, inner.width.saturating_sub(2) as usize),
            style,
        );
        y += 1;
    }
    // 修復建議
    for hint in crate::collectors::gpu::nvidia::repair_hints() {
        if y >= inner.y + inner.height {
            break;
        }
        buf.set_string(
            inner.x,
            y,
            format::truncate(hint, inner.width as usize),
            theme.faint_style(),
        );
        y += 1;
    }
}
