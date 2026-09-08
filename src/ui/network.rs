//! Network 頁面。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::app::App;
use crate::metrics::model::Severity;
use crate::ui::common::{history_graph, KeyValues};
use crate::ui::format;
use crate::ui::layout::{cols, split_top};
use crate::ui::widgets::braille::GraphStyle;
use crate::ui::widgets::panel::{inset, Panel};

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let n = app.state.network.state();
    let theme = &app.theme;

    let graph_h = (area.height / 4).clamp(5, 12);
    let (top, rest) = split_top(area, graph_h);

    // 上下鏡像的雙向波形。
    //
    // 兩張分開的圖，讀者得自己在腦裡對齊時間軸才知道「這個尖峰是同時發生的嗎」。
    // 鏡像之後同一個時刻的收與送就在同一欄上下相對，
    // 「這是雙向流量還是單向下載」變成一眼可見。
    // 畫面太矮時退回原本的兩張線圖，那時鏡像只剩一列，反而讀不出來。
    // 6 列就夠：面板框佔 2 列，剩 4 列讓上下各 2 列的波形站得住。
    // 再矮就只剩一列，鏡像反而讀不出來，那時退回兩張線圖比較誠實。
    let mirrored_ok = top.height >= 6 && top.width >= 40;
    if mirrored_ok {
        let inner = Panel::new(theme, "Throughput")
            .subtitle(format!(
                "↓ {}   ↑ {}",
                format::reading(&n.total_rx),
                format::reading(&n.total_tx)
            ))
            .render(top, buf);
        app.regions.add_panel(top, "net.throughput", "Throughput");
        let inner = inset(inner, 1, 0);
        // Series 是環狀緩衝，這裡只取畫得下的最後幾個點，不整份複製
        let take = inner.width as usize;
        let tx: Vec<f64> = n.tx_history.tail(take).collect();
        let rx: Vec<f64> = n.rx_history.tail(take).collect();
        crate::ui::widgets::viz::mirrored(
            buf,
            inner,
            theme,
            &tx,
            &rx,
            theme.style(theme.palette.warning),
            theme.style(theme.palette.ok),
        );
        // 標出哪半邊是哪個方向，不然鏡像圖看起來像雜訊
        if inner.height >= 4 {
            buf.set_string(inner.x, inner.y, "↓ down", theme.style(theme.palette.ok));
            buf.set_string(
                inner.x,
                inner.y + inner.height - 1,
                "↑ up",
                theme.style(theme.palette.warning),
            );
        }
    } else {
        let halves = cols(top, &[1, 1]);
        let inner = Panel::new(theme, "Download")
            .subtitle(format::reading(&n.total_rx))
            .render(halves[0], buf);
        app.regions
            .add_panel(halves[0], "net.throughput", "Download");
        history_graph(
            buf,
            inset(inner, 1, 0),
            theme,
            &n.rx_history,
            theme.c(theme.palette.ok),
            None,
            format::bytes,
            GraphStyle::Line,
        );
        let inner = Panel::new(theme, "Upload")
            .subtitle(format::reading(&n.total_tx))
            .render(halves[1], buf);
        app.regions.add_panel(halves[1], "net.throughput", "Upload");
        history_graph(
            buf,
            inset(inner, 1, 0),
            theme,
            &n.tx_history,
            theme.c(theme.palette.warning),
            None,
            format::bytes,
            GraphStyle::Line,
        );
    }

    // 介面：實體的排前面
    let mut ifaces: Vec<_> = n.interfaces.iter().collect();
    ifaces.sort_by_key(|i| (i.virtual_iface, !i.up, i.name.clone()));

    let (list, sockets) = split_top(rest, rest.height.saturating_sub(6));
    let mut y = list.y;
    for it in &ifaces {
        if y + 5 > list.y + list.height {
            break;
        }
        render_iface(
            app,
            Rect {
                y,
                height: 6.min((list.y + list.height).saturating_sub(y)),
                ..list
            },
            buf,
            it,
        );
        y += 6;
    }
    if sockets.height >= 3 {
        render_sockets(app, sockets, buf);
    }
}

fn render_iface(
    app: &App,
    area: Rect,
    buf: &mut Buffer,
    it: &crate::collectors::network::Interface,
) {
    let theme = &app.theme;
    let mut sub = if it.up {
        "UP".to_owned()
    } else {
        "DOWN".to_owned()
    };
    if let Some(s) = it.speed_mbit {
        sub.push_str(&format!(" · {s} Mb/s"));
    }
    let title_style = if it.up {
        theme.bold(theme.palette.network)
    } else {
        theme.faint_style()
    };
    let inner = Panel::new(theme, &it.name)
        .subtitle(sub)
        .title_style(title_style)
        .render(area, buf);
    let panel = app.regions.add_entity_panel(
        area,
        crate::metrics::describe::EntityRef::NetworkInterface(it.name.clone()),
        it.name.as_str(),
    );
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    let parts = cols(inner, &[5, 4]);
    let err_sev = if it.errors + it.drops > 0 {
        Severity::Warning
    } else {
        Severity::Ok
    };
    KeyValues::new(theme)
        .focusable(app, panel)
        .key_width(11)
        .row_of(
            "Download",
            format!(
                "{}   總計 {}",
                format::reading(&it.rx_rate),
                format::bytes(it.rx_total as f64)
            ),
            "net.throughput",
        )
        .row_of(
            "Upload",
            format!(
                "{}   總計 {}",
                format::reading(&it.tx_rate),
                format::bytes(it.tx_total as f64)
            ),
            "net.throughput",
        )
        .styled_row_of(
            "Errors/Drops",
            format!("{} {} / {}", err_sev.symbol(), it.errors, it.drops),
            theme.severity_style(err_sev),
            "net.errors",
        )
        .row_of(
            "MAC",
            it.mac.clone().unwrap_or_else(|| "—".into()),
            "net.mac",
        )
        .render(parts[0], buf);

    let mut kv = KeyValues::new(theme).focusable(app, panel).key_width(5);
    for (i, a) in it.addrs.iter().take(3).enumerate() {
        kv = kv.row_of(if i == 0 { "IP" } else { "" }, a.clone(), "net.ip");
    }
    if it.addrs.is_empty() {
        kv = kv.row_of("IP", "—", "net.ip");
    }
    kv.render(parts[1], buf);
}

fn render_sockets(app: &App, area: Rect, buf: &mut Buffer) {
    let s = &app.state.network.state().sockets;
    let theme = &app.theme;
    let inner = Panel::new(theme, "Sockets")
        .subtitle("完整的 socket → 行程對應需要管理員權限（按 A）")
        .render(area, buf);
    let panel = app.regions.add_panel(area, "net.sockets", "Sockets");
    let inner = inset(inner, 1, 0);
    let fmt = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_else(|| "—".into());
    KeyValues::new(theme)
        .focusable(app, panel)
        .key_width(16)
        .row_of("TCP in use", fmt(s.tcp_inuse), "net.sockets")
        .row_of("TCP TIME_WAIT", fmt(s.tcp_tw), "net.sockets")
        .row_of("TCP orphan", fmt(s.tcp_orphan), "net.sockets")
        .row_of("UDP in use", fmt(s.udp_inuse), "net.sockets")
        .row_of("Total sockets", fmt(s.sockets_used), "net.sockets")
        .render(inner, buf);
}
