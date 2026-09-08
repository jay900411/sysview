//! 管理員頁面。
//!
//! # 這一頁的規則
//!
//! * 沒有授權時顯示 `locked` 與怎麼解鎖，**不是**顯示空資料或假資料。
//! * 每個數字都標明是透過 `sudo sysview-priv` 取得的，來源完全透明。
//! * 掃描 `/home` 這類昂貴操作**按需執行**，而且會顯示進度、可取消、
//!   結果不完整時明講（`partial`）。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use crate::app::{AdminTab, App, Async, ADMIN_TABS};
use crate::metrics::model::Severity;
use crate::privilege::PrivilegeState;
use crate::ui::common::{placeholder, table_header, table_row, KeyValues};
use crate::ui::format;
use crate::ui::layout::split_top;
use crate::ui::widgets::gauge::Gauge;
use crate::ui::widgets::panel::{inset, Panel};

pub fn render(app: &App, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    match app.privilege.state() {
        PrivilegeState::Available => {}
        state => {
            render_locked(app, area, buf, &state);
            return;
        }
    }

    // Admin 有自己的視覺語言：專屬強調色 + 麵包屑 + 標記過的邊框。
    // 一眼就知道「現在看到的是別人的資料，而且是靠 sudo 拿到的」。
    let admin_accent = theme.style(theme.palette.notice);
    let (crumb_area, rest) = split_top(area, 1);
    let (tabs, body) = split_top(rest, 1);

    let mut crumb = format!("{} ADMIN", crate::ui::visual::motif::PREFIX);
    crumb.push_str(&format!("  ›  {}", app.admin.tab().title()));
    if let Some(u) = &app.admin.detail_user {
        crumb.push_str(&format!("  ›  {u}"));
    }
    buf.set_string(
        crumb_area.x + 1,
        crumb_area.y,
        format::truncate(&crumb, crumb_area.width.saturating_sub(2) as usize),
        theme.bold(theme.palette.notice),
    );
    // 右側標明資料來源，因為這一頁的資料跟其他頁不是同一個管道來的
    let src = "root via sudo sysview-priv";
    let sw = format::width(src) as u16;
    if crumb_area.width > format::width(&crumb) as u16 + sw + 4 {
        buf.set_string(
            crumb_area.x + crumb_area.width - sw - 1,
            crumb_area.y,
            src,
            theme.faint_style(),
        );
    }

    let active_tab = render_tabs(app, tabs, buf);

    let inner = Panel::new(theme, app.admin.tab().title())
        .subtitle("透過 sudo sysview-priv 取得")
        .border_style(admin_accent)
        .render(body, buf);
    // 清單的列直接掛在「目前這個分頁」底下。
    //
    // 中間不再放一個內容面板：那一層跟分頁同名，按 Enter 看起來像沒反應，
    // 卻要多按一次才會到列。分頁本身就代表這份清單，所以在分頁上按 Enter
    // 就直接進到第一列，也不必再記 [ ] 這種鍵。
    let panel = match active_tab {
        Some(t) => t,
        None => {
            app.regions
                .add_list_panel(body, app.admin.tab().metric_id(), app.admin.tab().title())
        }
    };
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }

    match app.admin.current() {
        Async::Idle => placeholder(buf, inner, theme, "按 r 開始查詢"),
        Async::Running { since } => {
            let secs = since.elapsed().as_secs();
            let msg = match app.admin.tab() {
                AdminTab::Storage => format!(
                    "正在掃描各使用者家目錄…（{secs}s）\n這是按需執行的完整走訪，大型家目錄需要一點時間"
                ),
                _ => format!("查詢中…（{secs}s）"),
            };
            placeholder(buf, inner, theme, &msg.replace('\n', "  "));
        }
        Async::Failed(e) => {
            // 缺授權跟真的出錯是兩回事，要給不同的指示
            let needs_auth = e.contains("授權") || e.contains("sudo");
            let (icon, style) = if needs_auth {
                ("!", theme.bold(theme.palette.warning))
            } else {
                ("✗", theme.bold(theme.palette.critical))
            };
            buf.set_string(
                inner.x,
                inner.y,
                format::truncate(&format!("{icon} {e}"), inner.width as usize),
                style,
            );
            if needs_auth && inner.height > 2 {
                buf.set_string(
                    inner.x,
                    inner.y + 2,
                    format::truncate("按 [u] 輸入密碼解鎖，或按 [r] 重試", inner.width as usize),
                    theme.style(theme.palette.accent),
                );
            }
        }
        Async::Ready(v) => match app.admin.tab() {
            AdminTab::Storage => render_storage(app, inner, buf, v, panel),
            AdminTab::Memory => render_memory(app, inner, buf, v, panel),
            AdminTab::Gpu => render_gpu(app, inner, buf, v, panel),
            AdminTab::Network => render_sockets(app, inner, buf, v, panel),
        },
    }
}

fn render_locked(app: &App, area: Rect, buf: &mut Buffer, state: &PrivilegeState) {
    let theme = &app.theme;
    let inner = Panel::new(theme, "Administrator Extensions")
        .subtitle(state.label())
        .border_style(theme.style(theme.palette.warning))
        .render(area, buf);
    app.regions
        .add_panel(area, "storage.user", "Administrator Extensions");
    let inner = inset(inner, 2, 1);
    if inner.height == 0 {
        return;
    }

    let lines: Vec<(&str, ratatui::style::Style)> = match state {
        PrivilegeState::Locked => vec![
            (
                "管理員擴充功能目前是鎖住的。",
                theme.bold(theme.palette.warning),
            ),
            ("", theme.dim_style()),
            (
                "按 [u] 解鎖。sysview 會暫時把終端機交給 sudo，",
                theme.style(theme.palette.fg),
            ),
            (
                "由 sudo 自己處理密碼提示 —— sysview 不會、也無法讀到你的密碼。",
                theme.style(theme.palette.fg),
            ),
            ("", theme.dim_style()),
            ("解鎖後可以看到：", theme.dim_style()),
            ("  · 各使用者的家目錄用量與明細", theme.dim_style()),
            ("  · 各使用者的記憶體用量（含 PSS）", theme.dim_style()),
            ("  · 各使用者的 GPU VRAM 佔用", theme.dim_style()),
            ("  · socket → 行程 → 使用者 對應", theme.dim_style()),
            ("", theme.dim_style()),
            ("整支 sysview 永遠以你自己的身分執行，", theme.faint_style()),
            (
                "只有一個極小的 helper（sysview-priv）會經 sudo 取得 root。",
                theme.faint_style(),
            ),
        ],
        PrivilegeState::Unauthorized(msg) => vec![
            (
                "你沒有執行管理員功能的權限。",
                theme.bold(theme.palette.critical),
            ),
            ("", theme.dim_style()),
            (
                "這不是錯誤 —— 是系統的 sudo 政策正確地拒絕了這個請求。",
                theme.style(theme.palette.fg),
            ),
            (
                "若你認為應該有權限，請聯絡管理員檢查 sudoers 設定。",
                theme.dim_style(),
            ),
            ("", theme.dim_style()),
            (msg.as_str(), theme.faint_style()),
        ],
        PrivilegeState::HelperMissing(msg) => vec![
            (
                "找不到 sysview-priv helper。",
                theme.bold(theme.palette.warning),
            ),
            ("", theme.dim_style()),
            (
                "一般使用的所有功能都不受影響，只有管理員擴充需要它。",
                theme.style(theme.palette.fg),
            ),
            (
                "執行 ./install.sh 或 sudo make install 即可安裝。",
                theme.dim_style(),
            ),
            ("", theme.dim_style()),
            (msg.as_str(), theme.faint_style()),
        ],
        PrivilegeState::Available => vec![],
    };
    // 文字在左，其他留白。吉祥物**不在這裡畫** —— 交給常駐浮水印那一層，
    // 跟其他頁同一套：露多少由乾淨區域決定，切頁時一起長大縮小。
    // 早期這裡自己放一隻固定大小的小狐狸（`slot::mascot`），結果從別頁
    // 切過來永遠是那麼小，看起來像「沒長回來」；而浮水印層看到這一頁
    // 已經有一隻就不畫，兩邊都動不了。
    for (i, (text, style)) in lines.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        buf.set_string(
            inner.x,
            y,
            format::truncate(text, inner.width as usize),
            *style,
        );
    }

    // 底部的識別列
    use crate::ui::visual::{motif, Decoration};
    let deco = app.deco();
    let sig_y = inner.y + inner.height.saturating_sub(1);
    if deco.at_least(Decoration::Minimal) && sig_y > inner.y + lines.len() as u16 {
        buf.set_string(
            inner.x,
            sig_y,
            format::truncate(
                &format!(
                    "{} PRIVILEGED EXTENSIONS  {}",
                    motif::PREFIX,
                    motif::SIGNATURE
                ),
                inner.width as usize,
            ),
            theme.faint_style(),
        );
    }
}

/// 分頁列。**每個分頁本身就是焦點樹的第一層** ——
/// 進入 Admin 頁按 Enter 就停在分頁上，用 ← → 換分頁，
/// 再按 Enter 才進到那個分頁的清單。不需要記 `[` `]` 這種另外的鍵。
fn render_tabs(app: &App, area: Rect, buf: &mut Buffer) -> Option<usize> {
    let theme = &app.theme;
    let mut x = area.x + 1;
    let mut active_region = None;
    for (i, tab) in ADMIN_TABS.iter().enumerate() {
        let active = i == app.admin.tab_index;
        let label = format!(" {} ", tab.title());
        let w = format::width(&label) as u16;
        if x + w > area.x + area.width {
            break;
        }
        let style = if active {
            theme
                .bold(theme.palette.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            theme.dim_style()
        };
        buf.set_string(x, area.y, &label, style);
        let rect = Rect {
            x,
            y: area.y,
            width: w,
            height: 1,
        };
        // 目前這個分頁底下掛著一份可捲動的清單，所以要登記成 list panel ——
        // 這樣在列上走到頭尾時才會改成捲動，而不是繞回第一列。
        let idx = if active {
            app.regions
                .add_list_panel(rect, tab.metric_id(), tab.title())
        } else {
            app.regions.add_panel(rect, tab.metric_id(), tab.title())
        };
        if active {
            active_region = Some(idx);
        }
        x += w + 1;
    }
    let hint = "← → 換分頁   Enter 進入   r 重新查詢";
    let hx = area.x + area.width.saturating_sub(format::width(hint) as u16 + 1);
    if hx > x {
        buf.set_string(hx, area.y, hint, theme.faint_style());
    }
    active_region
}

/// 清單的可見視窗：讓選取的那一列盡量留在畫面中央。
///
/// 四個管理員分頁共用同一套，選取行為才會一致 —— 之前只有 User Storage
/// 會捲動，另外三頁超過面板高度的列根本選不到。
fn window(selected: usize, len: usize, visible: usize) -> std::ops::Range<usize> {
    let start = crate::ui::layout::clamp_scroll(selected.saturating_sub(visible / 2), len, visible);
    start..(start + visible).min(len)
}

fn render_storage(app: &App, area: Rect, buf: &mut Buffer, v: &serde_json::Value, panel: usize) {
    let theme = &app.theme;
    let Some(users) = v.get("users").and_then(|u| u.as_array()) else {
        placeholder(buf, area, theme, "沒有資料");
        return;
    };
    let total: u64 = users.iter().filter_map(|u| u["bytes"].as_u64()).sum();

    // 版面：清單 → （可選）waffle → （可選）明細
    //
    // waffle 回答的是清單答不了的問題：「這幾個人加起來佔了多少」。
    // 一整欄百分比數字要讀者自己在腦裡加總，格圖直接把比例攤在眼前。
    let want_waffle = app.deco().at_least(crate::ui::visual::Decoration::Minimal)
        && users.len() >= 2
        && area.height >= 14;
    let waffle_h = if want_waffle { 5 } else { 0 };
    let (list_area, detail_area) = if app.admin.detail_user.is_some() && area.height > 10 {
        split_top(area, area.height / 2)
    } else {
        (area, Rect { height: 0, ..area })
    };
    let (list_area, waffle_area) = if want_waffle && list_area.height > waffle_h + 3 {
        let (l, w) = split_top(list_area, list_area.height - waffle_h);
        (l, Some(w))
    } else {
        (list_area, None)
    };

    table_header(
        buf,
        list_area,
        theme,
        &[
            ("USER", 14, false),
            ("HOME", 22, false),
            ("USED", 11, true),
            ("FILES", 9, true),
            ("SHARE", 18, false),
            ("", 10, false),
        ],
    );
    let visible = list_area.height.saturating_sub(1) as usize;
    let win = window(app.admin.selected, users.len(), visible);

    for (i, u) in users[win.clone()].iter().enumerate() {
        let y = list_area.y + 1 + i as u16;
        let idx = win.start + i;
        let selected = idx == app.admin.selected;
        let bytes = u["bytes"].as_u64().unwrap_or(0);
        let share = if total > 0 {
            bytes as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        let complete = u["complete"].as_bool().unwrap_or(true);

        let name_style = if selected {
            theme
                .bold(theme.palette.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            theme.style(theme.palette.fg)
        };
        app.regions.add_row(
            panel,
            Rect {
                x: list_area.x,
                y,
                width: list_area.width,
                height: 1,
            },
            "storage.user",
            u["user"].as_str().unwrap_or("?").to_owned(),
            idx,
        );
        table_row(
            buf,
            Rect {
                y,
                height: 1,
                ..list_area
            },
            &[
                (
                    u["user"].as_str().unwrap_or("?").to_owned(),
                    14,
                    false,
                    name_style,
                ),
                (
                    u["home"].as_str().unwrap_or("").to_owned(),
                    22,
                    false,
                    theme.faint_style(),
                ),
                (
                    format::bytes(bytes as f64),
                    11,
                    true,
                    theme.style(theme.palette.disk),
                ),
                (
                    format::count(u["entries"].as_f64().unwrap_or(0.0)),
                    9,
                    true,
                    theme.dim_style(),
                ),
                (String::new(), 18, false, theme.dim_style()),
            ],
        );
        let bar_x = list_area.x + 14 + 22 + 11 + 9 + 4;
        if bar_x + 12 < list_area.x + list_area.width {
            Gauge::new(theme, share).render(
                Rect {
                    x: bar_x,
                    y,
                    width: 12,
                    height: 1,
                },
                buf,
            );
            buf.set_string(
                bar_x + 13,
                y,
                format!("{share:>4.0}%"),
                theme.severity_style(Severity::from_percent(share)),
            );
            // 掃描不完整一定要講清楚，不能讓使用者誤信數字
            if !complete {
                let x = bar_x + 19;
                if x + 8 < list_area.x + list_area.width {
                    buf.set_string(x, y, "partial", theme.style(theme.palette.warning));
                }
            }
        }
    }

    if let Some(w) = waffle_area {
        render_user_waffle(app, w, buf, users);
    }
    if detail_area.height > 0 {
        render_storage_detail(app, detail_area, buf);
    }
}

/// 使用者照**用量**由大到小排。
///
/// 原本是直接 `take(5)`，而清單是照名字排的 —— 於是圖例顯示的是名字最
/// 前面的五個人（常常都是 0%），真正佔掉磁碟的那個人被併進「其他 N 人
/// 88%」。這張圖的用途就是「誰佔了空間」，照名字取前五名等於把答案藏起來。
fn by_usage(users: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    let mut ranked: Vec<&serde_json::Value> = users.iter().collect();
    ranked.sort_by(|a, b| {
        b["bytes"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["bytes"].as_u64().unwrap_or(0))
    });
    ranked
}

/// 各使用者佔比的格圖。
///
/// 選 waffle 而不是 treemap：treemap 要在整數格上切矩形，用量小的人會被
/// 捨入成 0 寬而整個消失 —— 在終端機這種粗格線的畫布上那是常態。
/// waffle 每格一樣大，每個人至少一格，「誰佔了多少」直接可數。
/// 使用者格圖的顏色，依用量名次分配。
///
/// **不用 `accent`**：那是系統線段（磁碟用量條、選取列）的顏色，第一名的
/// 使用者塗同一色會看起來像系統的一部分而不是某個人（使用者回報）。也不用
/// `network`：在 256 色主題裡它跟 accent 都是青色，兩個人會撞在一起。
/// 候選色照主題實際的值篩：dracula 的 memory 就是 accent 那個紫，寫死一組
/// 欄位名在那裡會撞色，所以撞到的跳過、重複的跳過，取前五個。
fn user_palette(theme: &crate::theme::Theme) -> Vec<ratatui::style::Color> {
    let p = &theme.palette;
    let mut out = Vec::with_capacity(5);
    // 後面幾個是備胎：dracula 的 memory 是 accent、ok 是 gpu、process 是
    // warning，前七個只湊得出四個。
    for c in [
        p.gpu,
        p.memory,
        p.warning,
        p.disk,
        p.critical,
        p.process,
        p.ok,
        p.notice,
        p.cpu,
        p.estimated,
        p.dim,
    ] {
        if c != p.accent && c != p.network && !out.contains(&c) {
            out.push(c);
        }
        if out.len() == 5 {
            break;
        }
    }
    out
}

fn render_user_waffle(app: &App, area: Rect, buf: &mut Buffer, users: &[serde_json::Value]) {
    use crate::ui::visual::pattern::COMPOSITION_ORDER;
    use crate::ui::widgets::viz;

    let theme = &app.theme;
    if area.height < 3 {
        return;
    }
    // 只畫前幾名，其餘合併成「其他」—— 幾十個人各一種紋理只會變成雜訊
    const TOP: usize = 5;
    let palette = user_palette(theme);
    let ranked = by_usage(users);
    let mut slices: Vec<viz::Slice> = Vec::new();
    for (i, u) in ranked.iter().take(TOP).enumerate() {
        slices.push(viz::Slice {
            label: u["user"].as_str().unwrap_or("?").to_owned(),
            value: u["bytes"].as_u64().unwrap_or(0) as f64,
            pattern: COMPOSITION_ORDER[i % COMPOSITION_ORDER.len()],
            style: theme.style(palette[i % palette.len()]),
        });
    }
    let rest: f64 = ranked
        .iter()
        .skip(TOP)
        .map(|u| u["bytes"].as_u64().unwrap_or(0) as f64)
        .sum();
    if rest > 0.0 {
        slices.push(viz::Slice {
            label: format!("其他 {} 人", users.len() - TOP),
            value: rest,
            pattern: COMPOSITION_ORDER[TOP % COMPOSITION_ORDER.len()],
            style: theme.faint_style(),
        });
    }
    // 格圖與圖例之間空一列。
    //
    // 跟記憶體頁的組成條同一個問題：格圖本身就是一整片實心方塊，
    // 圖例緊貼在下面時，圖例開頭的 █ 會跟上面那排連成一片，
    // 看起來像格圖多了一列（使用者回報過兩次）。
    let gap = u16::from(area.height >= 5);
    let (grid, legend) = split_top(area, area.height.saturating_sub(1 + gap));
    let legend = Rect {
        y: legend.y + gap,
        height: legend.height.saturating_sub(gap),
        ..legend
    };
    viz::waffle(buf, grid, &slices, theme.faint_style());

    // 圖例：紋理 + 名字 + 百分比。沒有它，格圖只是一片花紋。
    let total: f64 = slices.iter().map(|s| s.value).sum::<f64>().max(1.0);
    let mut x = legend.x;
    for s in &slices {
        let text = format!(
            "{} {} {:.0}%",
            viz::waffle_glyph(s.pattern),
            s.label,
            s.value / total * 100.0
        );
        let w = format::width(&text) as u16 + 2;
        if x + w > legend.x + legend.width {
            break;
        }
        buf.set_string(x, legend.y, &text, s.style);
        x += w;
    }
}

fn render_storage_detail(app: &App, area: Rect, buf: &mut Buffer) {
    let theme = &app.theme;
    let user = app.admin.detail_user.clone().unwrap_or_default();
    let inner = Panel::new(theme, &format!("~{user} breakdown")).render(area, buf);
    let inner = inset(inner, 1, 0);
    if inner.height == 0 {
        return;
    }
    match &app.admin.detail {
        Async::Running { since } => placeholder(
            buf,
            inner,
            theme,
            &format!(
                "掃描 {user} 的家目錄中…（{}s，按 Esc 可離開）",
                since.elapsed().as_secs()
            ),
        ),
        Async::Failed(e) => {
            buf.set_string(
                inner.x,
                inner.y,
                format::truncate(e, inner.width as usize),
                theme.style(theme.palette.critical),
            );
        }
        Async::Ready(v) => {
            let total = v["bytes"].as_u64().unwrap_or(1).max(1);
            let empty = vec![];
            let children = v["children"].as_array().unwrap_or(&empty);
            for (i, c) in children.iter().take(inner.height as usize).enumerate() {
                let y = inner.y + i as u16;
                let b = c["bytes"].as_u64().unwrap_or(0);
                let pct = b as f64 / total as f64 * 100.0;
                let name = c["name"].as_str().unwrap_or("?");
                buf.set_string(
                    inner.x,
                    y,
                    format::pad(&format!("~/{name}"), 26),
                    theme.dim_style(),
                );
                buf.set_string(
                    inner.x + 27,
                    y,
                    format::pad_left(&format::bytes(b as f64), 11),
                    theme.style(theme.palette.disk),
                );
                let bx = inner.x + 39;
                if bx + 14 < inner.x + inner.width {
                    Gauge::new(theme, pct).render(
                        Rect {
                            x: bx,
                            y,
                            width: 12,
                            height: 1,
                        },
                        buf,
                    );
                }
            }
            if children.is_empty() {
                placeholder(buf, inner, theme, "沒有子目錄");
            }
        }
        Async::Idle => placeholder(buf, inner, theme, "按 Enter 展開"),
    }
}

fn render_memory(app: &App, area: Rect, buf: &mut Buffer, v: &serde_json::Value, panel: usize) {
    let theme = &app.theme;
    let Some(users) = v.get("users").and_then(|u| u.as_array()) else {
        placeholder(buf, area, theme, "沒有資料");
        return;
    };
    table_header(
        buf,
        area,
        theme,
        &[
            ("USER", 14, false),
            ("RSS", 12, true),
            ("PSS", 12, true),
            ("PROCS", 8, true),
        ],
    );
    let win = window(
        app.admin.selected,
        users.len(),
        area.height.saturating_sub(1) as usize,
    );
    for (i, u) in users[win.clone()].iter().enumerate() {
        let y = area.y + 1 + i as u16;
        let idx = win.start + i;
        let pss = match u["pss"].as_u64() {
            Some(p) => format::bytes(p as f64),
            // 核心執行緒與部分行程沒有 smaps_rollup，不能假裝有值
            None => "n/a".into(),
        };
        app.regions.add_row(
            panel,
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
            "proc.rss",
            u["user"].as_str().unwrap_or("?").to_owned(),
            idx,
        );
        // 跟 User Storage 一樣，選到的那一列要看得出來
        let name_style = if idx == app.admin.selected {
            theme
                .bold(theme.palette.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            theme.style(theme.palette.fg)
        };
        table_row(
            buf,
            Rect {
                y,
                height: 1,
                ..area
            },
            &[
                (
                    u["user"].as_str().unwrap_or("?").to_owned(),
                    14,
                    false,
                    name_style,
                ),
                (
                    format::bytes(u["rss"].as_f64().unwrap_or(0.0)),
                    12,
                    true,
                    theme.style(theme.palette.memory),
                ),
                (pss, 12, true, theme.style(theme.palette.accent)),
                (
                    u["processes"].as_u64().unwrap_or(0).to_string(),
                    8,
                    true,
                    theme.dim_style(),
                ),
            ],
        );
    }
    let note = "PSS 把共享頁面按比例分攤，比 RSS 公平（RSS 會重複計算共享函式庫）";
    let y = area.y + area.height.saturating_sub(1);
    buf.set_string(
        area.x,
        y,
        format::truncate(note, area.width as usize),
        theme.faint_style(),
    );
}

fn render_gpu(app: &App, area: Rect, buf: &mut Buffer, v: &serde_json::Value, panel: usize) {
    let theme = &app.theme;
    if !v["available"].as_bool().unwrap_or(false) {
        let reason = v["reason"].as_str().unwrap_or("NVML 不可用");
        placeholder(buf, area, theme, reason);
        return;
    }
    let (top, bottom) = split_top(area, area.height / 2);
    let empty = vec![];
    let users = v["users"].as_array().unwrap_or(&empty);
    table_header(
        buf,
        top,
        theme,
        &[("USER", 14, false), ("VRAM", 12, true), ("PROCS", 8, true)],
    );
    let win = window(
        app.admin.selected,
        users.len(),
        top.height.saturating_sub(1) as usize,
    );
    for (i, u) in users[win.clone()].iter().enumerate() {
        let idx = win.start + i;
        app.regions.add_row(
            panel,
            Rect {
                x: top.x,
                y: top.y + 1 + i as u16,
                width: top.width,
                height: 1,
            },
            "gpu.vram",
            u["user"].as_str().unwrap_or("?").to_owned(),
            idx,
        );
        // 跟 User Storage 一樣，選到的那一列要看得出來
        let name_style = if idx == app.admin.selected {
            theme
                .bold(theme.palette.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            theme.style(theme.palette.fg)
        };
        table_row(
            buf,
            Rect {
                y: top.y + 1 + i as u16,
                height: 1,
                ..top
            },
            &[
                (
                    u["user"].as_str().unwrap_or("?").to_owned(),
                    14,
                    false,
                    name_style,
                ),
                (
                    format::bytes(u["vram"].as_f64().unwrap_or(0.0)),
                    12,
                    true,
                    theme.style(theme.palette.gpu),
                ),
                (
                    u["processes"].as_u64().unwrap_or(0).to_string(),
                    8,
                    true,
                    theme.dim_style(),
                ),
            ],
        );
    }
    if users.is_empty() {
        placeholder(buf, top, theme, "目前沒有人在用 GPU");
    }

    // 下半：逐一列出行程
    table_header(
        buf,
        bottom,
        theme,
        &[
            ("GPU", 5, true),
            ("PID", 8, true),
            ("USER", 12, false),
            ("VRAM", 11, true),
            ("PROCESS", 24, false),
        ],
    );
    let mut y = bottom.y + 1;
    for dev in v["devices"].as_array().unwrap_or(&empty) {
        for p in dev["processes"].as_array().unwrap_or(&empty) {
            if y >= bottom.y + bottom.height {
                return;
            }
            table_row(
                buf,
                Rect {
                    y,
                    height: 1,
                    ..bottom
                },
                &[
                    (
                        dev["index"].as_u64().unwrap_or(0).to_string(),
                        5,
                        true,
                        theme.faint_style(),
                    ),
                    (
                        p["pid"].as_u64().unwrap_or(0).to_string(),
                        8,
                        true,
                        theme.faint_style(),
                    ),
                    (
                        p["user"].as_str().unwrap_or("?").to_owned(),
                        12,
                        false,
                        theme.dim_style(),
                    ),
                    (
                        format::bytes(p["used_memory"].as_f64().unwrap_or(0.0)),
                        11,
                        true,
                        theme.style(theme.palette.gpu),
                    ),
                    (
                        p["name"].as_str().unwrap_or("?").to_owned(),
                        24,
                        false,
                        theme.style(theme.palette.fg),
                    ),
                ],
            );
            y += 1;
        }
    }
}

fn render_sockets(app: &App, area: Rect, buf: &mut Buffer, v: &serde_json::Value, panel: usize) {
    let theme = &app.theme;
    let empty = vec![];
    let users = v["users"].as_array().unwrap_or(&empty);
    table_header(
        buf,
        area,
        theme,
        &[
            ("USER", 14, false),
            ("TCP EST", 9, true),
            ("LISTEN", 8, true),
            ("TIME_WAIT", 11, true),
            ("UDP", 7, true),
            ("TOTAL", 8, true),
        ],
    );
    let win = window(
        app.admin.selected,
        users.len(),
        area.height.saturating_sub(2) as usize,
    );
    for (i, u) in users[win.clone()].iter().enumerate() {
        let idx = win.start + i;
        let g = |k: &str| u[k].as_u64().unwrap_or(0).to_string();
        app.regions.add_row(
            panel,
            Rect {
                x: area.x,
                y: area.y + 1 + i as u16,
                width: area.width,
                height: 1,
            },
            "net.sockets",
            u["user"].as_str().unwrap_or("?").to_owned(),
            idx,
        );
        // 跟 User Storage 一樣，選到的那一列要看得出來
        let name_style = if idx == app.admin.selected {
            theme
                .bold(theme.palette.accent)
                .add_modifier(Modifier::REVERSED)
        } else {
            theme.style(theme.palette.fg)
        };
        table_row(
            buf,
            Rect {
                y: area.y + 1 + i as u16,
                height: 1,
                ..area
            },
            &[
                (
                    u["user"].as_str().unwrap_or("?").to_owned(),
                    14,
                    false,
                    name_style,
                ),
                (
                    g("established"),
                    9,
                    true,
                    theme.style(theme.palette.network),
                ),
                (g("listen"), 8, true, theme.style(theme.palette.accent)),
                (g("time_wait"), 11, true, theme.dim_style()),
                (g("udp"), 7, true, theme.dim_style()),
                (g("total"), 8, true, theme.style(theme.palette.fg)),
            ],
        );
    }
    if users.is_empty() {
        placeholder(buf, area, theme, "沒有資料");
    }
    let y = area.y + area.height.saturating_sub(1);
    buf.set_string(
        area.x,
        y,
        format::truncate(
            "只顯示中繼資料（誰開了幾條連線）。sysview 不做封包擷取，也不看內容。",
            area.width as usize,
        ),
        theme.faint_style(),
    );
}

/// 二次確認視窗。**所有會改變系統狀態的操作都必須經過這裡。**
pub fn render_confirm(app: &App, area: Rect, buf: &mut Buffer, c: &crate::app::Confirmation) {
    use crate::ui::widgets::modal::{self, Cap};
    let theme = &app.theme;
    let win = crate::ui::layout::centered(area, 72, 16);
    let inner = inset(
        modal::frame(
            buf,
            theme,
            win,
            &modal::Modal {
                title: &c.title,
                subtitle: None,
                border: Some(theme.palette.critical),
                caps: &[Cap::Choose, Cap::CloseEsc],
            },
        ),
        1,
        1,
    );
    if inner.height < 6 {
        return;
    }

    KeyValues::new(theme)
        .key_width(12)
        .row("PID", c.target_pid.to_string())
        .row("User", c.target_user.clone())
        .row(
            "Command",
            format::truncate(&c.target_command, (inner.width as usize).saturating_sub(13)),
        )
        .row("Action", c.action.clone())
        .render(inner, buf);

    let mut y = inner.y + 5;
    if let Some(w) = &c.warning {
        buf.set_string(
            inner.x,
            y,
            format::truncate(&format!("⚠ {w}"), inner.width as usize),
            theme.bold(theme.palette.warning),
        );
        y += 2;
    }

    // 按鈕：預設停在「取消」上，必須主動移過去才能確認
    let cancel = "  取消  ";
    let confirm = "  確認執行  ";
    let cy = inner.y + inner.height.saturating_sub(1).max(y - inner.y);
    let sel = theme
        .bold(theme.palette.fg)
        .add_modifier(Modifier::REVERSED);
    let unsel = theme.dim_style();
    buf.set_string(
        inner.x,
        cy,
        cancel,
        if c.confirmed_selected { unsel } else { sel },
    );
    buf.set_string(
        inner.x + format::width(cancel) as u16 + 2,
        cy,
        confirm,
        if c.confirmed_selected {
            theme
                .bold(theme.palette.critical)
                .add_modifier(Modifier::REVERSED)
        } else {
            unsel
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_storage_legend_shows_the_biggest_users_not_the_first_alphabetically() {
        // 實際踩到的：圖例寫著五個 0–2% 的人，真正佔了 88% 的那個被併進
        // 「其他 2 人」。清單是照名字排的，而程式直接取前五個。
        let users: Vec<serde_json::Value> = [
            ("alice", 1u64),
            ("bob", 20),
            ("carol", 90),
            ("dave", 1),
            ("erin", 20),
            ("zzz_big", 8000),
            ("zzz_huge", 9000),
        ]
        .iter()
        .map(|(u, b)| serde_json::json!({ "user": u, "bytes": b }))
        .collect();

        let ranked = by_usage(&users);
        let top: Vec<&str> = ranked
            .iter()
            .take(2)
            .map(|u| u["user"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(
            top,
            vec!["zzz_huge", "zzz_big"],
            "圖例沒有把用量最大的排在前面"
        );
        // 前五名要涵蓋絕大部分的量，「其他」才會是小的那一塊
        let shown: u64 = ranked
            .iter()
            .take(5)
            .map(|u| u["bytes"].as_u64().unwrap_or(0))
            .sum();
        let total: u64 = users.iter().map(|u| u["bytes"].as_u64().unwrap_or(0)).sum();
        assert!(
            shown * 100 / total > 95,
            "前五名只涵蓋 {}%，最大的那些被藏進「其他」了",
            shown * 100 / total
        );
    }
}

#[cfg(test)]
mod waffle_tests {
    use super::*;
    use crate::theme::ColorDepth;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn user_colours_never_reuse_the_system_bar_colour() {
        // 使用者回報：第一名的 █ 跟系統線段同色，看起來像系統的一部分。
        // 256 色主題裡 network 跟 accent 也都是青色，一併避開。
        for name in crate::theme::THEMES {
            for depth in [ColorDepth::TrueColor, ColorDepth::Indexed256] {
                let theme = crate::theme::Theme::new(name, depth);
                let colours = user_palette(&theme);
                for &c in &colours {
                    assert_ne!(c, theme.palette.accent, "{name}: 使用者用了 accent");
                    assert_ne!(c, theme.palette.network, "{name}: 使用者用了 network");
                }
                assert_eq!(colours.len(), 5, "{name}: 湊不出五個顏色");
                for (i, a) in colours.iter().enumerate() {
                    for b in &colours[i + 1..] {
                        assert_ne!(a, b, "{name}: 兩個使用者同色");
                    }
                }
            }
        }
    }

    #[test]
    fn the_storage_legend_does_not_touch_the_waffle_above_it() {
        // 使用者回報過兩次的同一件事：格圖是一整片實心方塊，圖例緊貼在
        // 下面時，圖例開頭的 █ 跟上面那排連成一片。要空一列。
        let app = crate::app::App::new(crate::config::Config::default(), ColorDepth::TrueColor);
        let users: Vec<serde_json::Value> = (0..6)
            .map(|i| serde_json::json!({ "user": format!("u{i}"), "bytes": 1000u64 * (i + 1) }))
            .collect();
        let area = Rect::new(0, 0, 60, 8);
        let mut term = Terminal::new(TestBackend::new(60, 8)).unwrap();
        term.draw(|f| render_user_waffle(&app, area, f.buffer_mut(), &users))
            .unwrap();
        let text = crate::ui::buffer_text(term.backend().buffer());
        let lines: Vec<&str> = text.lines().collect();
        let legend = lines
            .iter()
            .rposition(|l| l.contains('%'))
            .expect("找不到圖例");
        assert!(legend >= 2, "圖例在第 {legend} 列，上面沒有格圖");
        assert!(
            lines[legend - 1].trim().is_empty(),
            "圖例上面那一列不是空的：{:?}",
            lines[legend - 1]
        );
        assert!(
            !lines[legend - 2].trim().is_empty(),
            "空一列就夠，不該空兩列"
        );
    }
}
