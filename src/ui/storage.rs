//! Storage 頁面。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use crate::app::App;
use crate::theme::Subsystem;
use crate::ui::common::{history_graph, table_header, table_row, KeyValues};
use crate::ui::format;
use crate::ui::layout::{cols, split_top};
use crate::ui::widgets::braille::GraphStyle;
use crate::ui::widgets::panel::{inset, Panel};

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let d = app.state.disk.state();
    let _theme = &app.theme;

    let mount_h = ((d.mounts.len() + 4) as u16).min(area.height / 2).max(5);
    let (top, rest) = split_top(area, mount_h);
    render_mounts(app, top, buf);

    if d.devices.is_empty() {
        return;
    }
    let per = (rest.height / d.devices.len().min(3) as u16).max(6);
    let mut y = rest.y;
    for dev in d.devices.iter().take(3) {
        if y + 5 > rest.y + rest.height {
            break;
        }
        let h = per.min(rest.y + rest.height - y).min(8);
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

fn render_mounts(app: &App, area: Rect, buf: &mut Buffer) {
    let d = app.state.disk.state();
    let theme = &app.theme;
    let inner = Panel::new(theme, "Filesystems")
        .subtitle(format!(
            "R {}  W {}",
            format::reading(&d.total_read),
            format::reading(&d.total_write)
        ))
        .render(area, buf);
    let panel = app
        .regions
        .add_list_panel(area, "disk.usage", "Filesystems");
    let inner = inset(inner, 1, 0);
    if inner.height < 2 {
        return;
    }

    // 依可用寬度決定要顯示哪些欄
    let wide = inner.width >= 96;
    let cols_def: Vec<(&str, u16, bool)> = if wide {
        vec![
            ("MOUNT", 22, false),
            ("DEVICE", 12, false),
            ("TYPE", 7, false),
            ("SIZE", 10, true),
            ("USED", 10, true),
            ("FREE", 10, true),
            ("USE%", 20, false),
            ("INODE", 7, true),
        ]
    } else {
        vec![
            ("MOUNT", 18, false),
            ("SIZE", 9, true),
            ("FREE", 9, true),
            ("USE%", 18, false),
        ]
    };
    table_header(buf, inner, theme, &cols_def);

    for (i, m) in d.mounts.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let sev = m.severity();
        let sev_style = theme.severity_style(sev);
        let dim = theme.dim_style();
        let fg = theme.style(theme.palette.fg);
        let disk = theme.style(theme.subsystem(Subsystem::Disk));

        let cells: Vec<(String, u16, bool, Style)> = if wide {
            vec![
                (m.mount_point.clone(), 22, false, disk),
                (
                    m.device.trim_start_matches("/dev/").to_owned(),
                    12,
                    false,
                    theme.faint_style(),
                ),
                (format!("[{}]", m.fs_type), 7, false, theme.faint_style()),
                (format::bytes(m.total as f64), 10, true, fg),
                (format::bytes(m.used as f64), 10, true, dim),
                (
                    format::bytes(m.available as f64),
                    10,
                    true,
                    theme.style(theme.palette.ok),
                ),
                (String::new(), 20, false, fg),
                (inode_text(&m.inode_usage), 7, true, dim),
            ]
        } else {
            vec![
                (m.mount_point.clone(), 18, false, disk),
                (format::bytes(m.total as f64), 9, true, fg),
                (
                    format::bytes(m.available as f64),
                    9,
                    true,
                    theme.style(theme.palette.ok),
                ),
                (String::new(), 18, false, fg),
            ]
        };
        table_row(
            buf,
            Rect {
                y,
                height: 1,
                ..inner
            },
            &cells,
        );
        // 每個掛載點都是一個可選的細項
        app.regions.add_entity_item(
            panel,
            Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 1,
            },
            crate::metrics::describe::EntityRef::Mount(m.mount_point.clone()),
            m.mount_point.clone(),
        );

        // USE% 欄畫成長條 + 符號 + 數字
        let bar_x = inner.x
            + cells[..cells.len() - 1 - usize::from(wide)]
                .iter()
                .map(|c| c.1 + 1)
                .sum::<u16>();
        let bar_w = if wide { 12 } else { 10 };
        if bar_x + bar_w + 7 <= inner.x + inner.width {
            // 用 pattern 畫「已用 / 剩餘」，不是只有顏色 ——
            // 顏色失效時（單色終端、色盲）長條仍然讀得出來
            use crate::ui::visual::pattern::{self, Pattern, Segment};
            let cells_bar = pattern::compose(
                &[
                    Segment {
                        label: "used".into(),
                        value: m.used as f64,
                        pattern: Pattern::Solid,
                        color: theme.c(theme.severity_color(sev)),
                    },
                    Segment {
                        label: "free".into(),
                        value: m.available as f64,
                        pattern: Pattern::Dot,
                        color: theme.c(theme.palette.faint),
                    },
                ],
                bar_w as usize,
                '·',
                theme.c(theme.palette.faint),
            );
            for (i, (ch, color)) in cells_bar.iter().enumerate() {
                buf.set_string(
                    bar_x + i as u16,
                    y,
                    ch.to_string(),
                    Style::default().fg(*color),
                );
            }
            buf.set_string(bar_x + bar_w + 1, y, sev.symbol(), sev_style);
            buf.set_string(
                bar_x + bar_w + 3,
                y,
                format::pad_left(&format!("{:.0}%", m.usage.or_zero()), 4),
                sev_style,
            );
        }
    }
}

/// inode 欄位很窄，"unsupported" 放不下會被截成 "unsupp…"，
/// 那看起來像壞掉。XFS/Btrfs 是動態配置 inode，顯示 n/a 才對。
fn inode_text(r: &crate::metrics::model::Reading) -> String {
    match r.get() {
        Some(_) => format::reading(r),
        None => "n/a".into(),
    }
}

fn render_device(app: &App, area: Rect, buf: &mut Buffer, dev: &crate::collectors::disk::Device) {
    let theme = &app.theme;
    // 種類做成 chip：在一堆數字裡一眼分出「這是分類不是量測值」
    let mut sub = format!(
        "[{}] {}",
        dev.kind(),
        format::bytes(dev.size.unwrap_or(0) as f64)
    );
    if let Some(t) = dev.temp.get() {
        sub.push_str(&format!(" · {t:.0}°C"));
    }
    let inner = Panel::new(theme, &format!("{} · {}", dev.name, dev.model))
        .subtitle(sub)
        .render(area, buf);
    let panel = app.regions.add_entity_panel(
        area,
        crate::metrics::describe::EntityRef::Disk(dev.name.clone()),
        dev.name.as_str(),
    );
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    let parts = cols(inner, &[4, 4, 3]);
    // 每一欄各自往內縮，讓圖表右側的座標軸不會貼到下一欄
    let gap = |r: Rect| Rect {
        width: r.width.saturating_sub(2),
        ..r
    };
    let (parts0, parts1, parts2) = (gap(parts[0]), gap(parts[1]), parts[2]);
    let parts = [parts0, parts1, parts2];
    history_graph(
        buf,
        parts[0],
        theme,
        &dev.read_history,
        theme.c(theme.palette.ok),
        None,
        format::bytes,
        GraphStyle::Line,
    );
    history_graph(
        buf,
        parts[1],
        theme,
        &dev.write_history,
        theme.c(theme.palette.warning),
        None,
        format::bytes,
        GraphStyle::Line,
    );
    buf.set_string(
        parts[0].x,
        parts[0].y,
        "read",
        theme.style(theme.palette.ok),
    );
    buf.set_string(
        parts[1].x,
        parts[1].y,
        "write",
        theme.style(theme.palette.warning),
    );

    KeyValues::new(theme)
        .focusable(app, panel)
        .key_width(9)
        .reading("Read", &dev.read_rate)
        .reading("Write", &dev.write_rate)
        .reading("IOPS", &dev.iops)
        .reading("Busy", &dev.util)
        .render(parts[2], buf);
}
