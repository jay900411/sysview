mod admin;
pub mod common;
pub mod cpu;
pub mod dino;
pub mod explain;
pub mod focus;
pub mod format;
pub mod gpu;
pub mod help;
pub mod layout;
pub mod memory;
pub mod network;
pub mod overview;
pub mod process;
pub mod storage;
pub mod visual;
pub mod widgets;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;

use crate::app::{App, Modal, View, VIEWS};
use crate::metrics::model::Severity;
use crate::privilege::PrivilegeState;

/// 畫整個畫面。這是 UI 的唯一入口。
pub fn draw(app: &App, frame: &mut ratatui::Frame) {
    let area = frame.area();
    // 這一幀的視覺層級由整個畫面決定，之後所有地方都讀這個值
    app.set_frame_metrics(area);
    // 吉祥物這一幀畫了沒？畫了才需要繼續跑動畫時鐘。
    app.begin_frame();
    let buf = frame.buffer_mut();
    // 先鋪底色，避免終端機原本的背景透出來造成主題不一致
    buf.set_style(
        area,
        ratatui::style::Style::default().bg(app.theme.c(app.theme.palette.bg)),
    );

    let (header, body, footer) = layout::chrome(area);
    render_header(app, header, buf);

    // 有可解釋的 metric 時，在內容區下方留一行畫「目前選到哪一個」。
    // 沒有這一行的話，↑↓ 明明有在動使用者也看不出來，`e` 也會看起來
    // 只能解釋固定的一個東西。
    // 每一幀重建可選區域：頁面會在 render_body 裡登記自己畫了什麼
    app.regions.clear();
    let show_focus = body.height > 6;
    let (content, focus_bar) = if show_focus {
        (
            Rect {
                height: body.height - 1,
                ..body
            },
            Some(Rect {
                y: body.y + body.height - 1,
                height: 1,
                ..body
            }),
        )
    } else {
        (body, None)
    };
    render_body(app, content, buf);
    // 常駐浮水印：所有監控頁共用的一層背景吉祥物。
    //
    // 一定要在 render_body **之後** —— 它是 masked layer，要看得到頁面
    // 已經畫了什麼才知道哪些格子是空的。也一定要在焦點框**之前**，
    // 因為焦點框永遠贏：選到的東西不能被一隻狐狸擋住。
    //
    // 有 modal 時整個不畫：畫了也看不見，卻會讓動畫時鐘繼續醒著。
    // 說明頁有自己的吉祥物，不需要這一層。
    //
    // `mascot_flag` 這時已經反映了頁面本身有沒有畫吉祥物（說明頁、啟動
    // 畫面）。一頁一隻就夠了，再疊一層浮水印會變成兩隻。管理員的鎖定畫面
    // 早期也自己畫一隻，現在交給這一層 —— 不然它永遠是固定的小尺寸，
    // 跟別頁切來切去像是「沒長回來」。
    if matches!(app.modal, Modal::None) && !app.mascot_flag().get() {
        if let Some((species, state)) = app.mascot_now() {
            crate::ui::visual::watermark::render(
                buf,
                content,
                &app.theme,
                crate::ui::visual::watermark::Watermark {
                    species,
                    state,
                    frame: app.anim_frame,
                    deco: app.watermark_deco(),
                    drawn: Some(app.mascot_flag()),
                    spot: Some(app.mascot_spot()),
                },
            );
        }
    }
    // 內容畫完之後才知道有哪些區域，這時再把焦點框疊上去
    render_focus_outline(app, buf);
    if let Some(bar) = focus_bar {
        render_focus_bar(app, bar, buf);
    }
    render_footer(app, footer, buf);

    // 視窗疊在最上面
    match &app.modal {
        Modal::None => {}
        Modal::Help => {
            // 說明頁固定用 observe：這裡不是在報告系統狀態
            let mascot = crate::ui::visual::mascot::Species::parse(&app.config.branding.mascot)
                .or(match app.config.branding.mascot.as_str() {
                    "none" => None,
                    _ => Some(crate::ui::visual::mascot::Species::Fox),
                })
                .map(|sp| (sp, crate::ui::visual::mascot::State::Observe));
            help::Help {
                theme: &app.theme,
                version: crate::VERSION,
                brand: Some(&app.brand),
                mascot,
                frame: app.anim_frame,
                deco: app.deco(),
                drawn: Some(app.mascot_flag()),
                scroll: app.help_scroll,
                max_scroll_out: Some(app.help_max_scroll()),
            }
            .render(area, buf)
        }
        Modal::Explain { target } => {
            // Describe 完全從已取樣的狀態組出來，不 fork 任何指令
            let content = crate::metrics::describe::describe(target, &app.state);
            let selected = app.focused_region().map(|r| r.label.clone());
            explain::Explain {
                content: &content,
                theme: &app.theme,
                scroll: app.explain_scroll,
                selected: selected.as_deref(),
                max_scroll_out: Some(&app.explain_max_scroll),
            }
            .render(area, buf);
        }
        Modal::Splash => render_splash(app, area, buf),
        Modal::Dino => render_dino(app, area, buf),
        Modal::Confirm(c) => admin::render_confirm(app, area, buf, c),
        Modal::Message { title, body } => render_message(app, area, buf, title, body),
    }
}

fn render_body(app: &App, area: Rect, buf: &mut Buffer) {
    match app.view {
        View::Overview => overview::render(app, area, buf),
        View::Cpu => cpu::render(app, area, buf),
        View::Memory => memory::render(app, area, buf),
        View::Gpu => gpu::render(app, area, buf),
        View::Storage => storage::render(app, area, buf),
        View::Network => network::render(app, area, buf),
        View::Process => process::render(app, area, buf),
        View::Admin => admin::render(app, area, buf),
    }
}

/// 整體健康狀態：把最糟的子系統狀況濃縮成一個徽章。
pub fn overall_health(app: &App) -> (Severity, String) {
    let mut worst = Severity::Ok;
    let mut reason = String::from("system healthy");

    let mut consider = |sev: Severity, why: String| {
        if sev > worst {
            worst = sev;
            reason = why;
        }
    };
    if let Some(p) = app.state.cpu.state().usage.get() {
        if p >= 90.0 {
            consider(Severity::Warning, format!("CPU {p:.0}%"));
        }
    }
    if let Some(p) = app.state.memory.state().used_percent().get() {
        if p >= 90.0 {
            consider(Severity::Critical, format!("memory {p:.0}%"));
        } else if p >= 80.0 {
            consider(Severity::Warning, format!("memory {p:.0}%"));
        }
    }
    for m in &app.state.disk.state().mounts {
        let sev = m.severity();
        if sev >= Severity::Warning {
            consider(sev, format!("{} {:.0}%", m.mount_point, m.usage.or_zero()));
        }
    }
    if app.state.gpu.state().nvidia_error.is_some() {
        consider(Severity::Warning, "NVIDIA driver unavailable".into());
    }
    let blocked = app.state.cpu.state().blocked.get().unwrap_or(0.0);
    if blocked > 0.0 {
        consider(Severity::Notice, format!("{blocked:.0} blocked on I/O"));
    }
    (worst, reason)
}

fn render_header(app: &App, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    let c = app.state.cpu.state();
    let deco = app.deco();

    // 第一列：品牌 + 識別資訊 + 健康狀態 + 時鐘
    let mut x = area.x + 1;
    buf.set_string(x, area.y, "▎", t.bold(t.palette.accent));
    x += 1;
    // 品牌代號取代 "sysview" 這個字，但產品身分不會消失 ——
    // 頁尾的標語與 --version 永遠講得出這是 sysview。
    let word = app.brand.word();
    buf.set_string(x, area.y, word, t.bold(t.palette.fg));
    x += format::width(word) as u16 + 1;
    let host = hostname();
    buf.set_string(x, area.y, &host, t.bold(t.palette.accent));
    x += format::width(&host) as u16 + 2;

    let os = os_name();
    let kernel = kernel_release();
    let env = app.state.cgroup.environment;
    let mut bits = vec![
        os,
        format!("Linux {kernel}"),
        format!("up {}", format::duration(c.uptime_secs)),
    ];
    // 只有在真的被限制（容器、有 CPU/記憶體配額）時才標示。
    // 一般 systemd user slice 也算 cgroup，但那對使用者沒有意義，標了只是噪音。
    if env == crate::collectors::cgroup::Environment::Container || app.state.cgroup.is_limited() {
        bits.push(format!("[{}]", env.label()));
    }
    for b in bits {
        let w = format::width(&b) as u16;
        if x + w + 34 > area.x + area.width {
            break;
        }
        buf.set_string(x, area.y, &b, t.dim_style());
        x += w + 2;
    }

    // 右側：健康徽章 + 時鐘
    let clock = clock_string();
    let cx = area.x + area.width.saturating_sub(format::width(&clock) as u16 + 1);
    buf.set_string(cx, area.y, &clock, t.bold(t.palette.fg));
    let (sev, why) = overall_health(app);
    let badge = format!("{} {why}", sev.symbol());
    let bx = cx.saturating_sub(format::width(&badge) as u16 + 3);
    if bx > x {
        buf.set_string(bx, area.y, &badge, t.severity_style(sev));
    }
    // 健康徽章與識別資訊之間若還有空間，補上快速狀態 chip
    if deco.at_least(crate::ui::visual::Decoration::Minimal) {
        let items = summary_chips(app);
        let need: u16 = items.iter().map(|(s, _)| format::width(s) as u16 + 3).sum();
        if bx > x + need + 2 {
            crate::ui::widgets::viz::chips(
                buf,
                Rect {
                    x: bx.saturating_sub(need + 2),
                    y: area.y,
                    width: need,
                    height: 1,
                },
                &items,
            );
        }
    }

    // 第二列：分頁
    let mut x = area.x + 1;
    for v in VIEWS {
        let label = format!(" {} {} ", v.key(), app.page_title(v, area.width));
        let w = format::width(&label) as u16;
        if x + w + 1 > area.x + area.width {
            break;
        }
        let active = v == app.view;
        let locked = v == View::Admin && !app.privilege.state().is_available();
        let style = if active {
            t.bold(t.palette.accent).add_modifier(Modifier::REVERSED)
        } else if locked {
            t.faint_style()
        } else {
            t.dim_style()
        };
        buf.set_string(x, area.y + 1, &label, style);
        x += w + 1;
    }
    // 分隔線，尾端帶上 concept 圖那種短橫簽名
    let remaining = area.width.saturating_sub(x - area.x) as usize;
    if remaining > 0 {
        let rule = if deco.at_least(crate::ui::visual::Decoration::Minimal) {
            crate::ui::visual::motif::rule(remaining)
        } else {
            "─".repeat(remaining)
        };
        buf.set_string(x, area.y + 1, rule, t.border_style());
    }
}

/// 頁首右側的快速狀態 chip：`CPU 12%  RAM 41%  GPU 0%`。
///
/// 只在寬度夠時出現。它不取代任何頁面上的數字，
/// 只是讓「不在那一頁時也知道那一頁大概怎樣」。
fn summary_chips(app: &App) -> Vec<(String, ratatui::style::Style)> {
    let t = &app.theme;
    let mut out = Vec::new();
    let mut push = |label: &str, v: Option<f64>| {
        let (text, style) = match v {
            Some(p) => (
                format!("{label} {p:.0}%"),
                t.severity_style(crate::metrics::model::Severity::from_percent(p)),
            ),
            None => (format!("{label} n/a"), t.style(t.palette.estimated)),
        };
        out.push((text, style));
    };
    push("CPU", app.state.cpu.state().usage.get());
    push("RAM", app.state.memory.state().used_percent().get());
    let gpu = app.state.gpu.state();
    if let Some(d) = gpu.devices.first() {
        push("GPU", d.utilization.get());
    }
    out
}

/// 啟動畫面。
///
/// 這是**唯一**可以整頁裝飾的地方 —— 這時候還沒有任何監控資料要讓位。
/// 它會自己消失，不需要使用者動手關。
/// 彩蛋畫面：中間浮一塊遊戲窗，底下的 dashboard 留著。
///
/// 早期版本整頁接管，看起來像切到另一個分頁而不是「在儀表板上面開了一局」。
/// 用跟說明頁、Explain 同一套 overlay 外框，一眼就看得出是同一個系統。
fn render_dino(app: &App, area: Rect, buf: &mut Buffer) {
    use crate::ui::widgets::modal::{self, Cap};
    let Some(g) = app.dino.as_ref() else { return };
    let t = &app.theme;
    let over = app.dino_over.as_ref();
    let entering = over.is_some_and(|o| o.typed.is_some());
    // 場地是「寬而扁」的 —— 恐龍跑酷本來就長這樣，而且留白很重要
    let w = dino::FIELD_W.min(area.width.saturating_sub(4));
    let h = (dino::FIELD_H + 4).min(area.height.saturating_sub(2));
    let win = layout::centered(area, w, h);
    let inner = modal::frame(
        buf,
        t,
        win,
        &modal::Modal {
            title: "DINO // BREAK",
            subtitle: Some(format!("最佳 {:05}", g.best())),
            border: None,
            // 提示要跟**現在**真的能做的事一致：死掉之後跳與蹲都沒有作用；
            // 名字輸入框開著時只有送出與不記錄
            caps: if entering {
                &[Cap::Name]
            } else if g.is_over() {
                &[Cap::Restart, Cap::CloseEsc]
            } else {
                &[Cap::Play, Cap::CloseEsc]
            },
        },
    );
    let panel = over.map(|o| dino::OverPanel {
        top: o.board.top(),
        shared: app.scores.is_shared(),
        typed: o.typed.as_deref(),
        note: o.note.as_deref(),
    });
    g.render(buf, inner, t, std::time::Instant::now(), panel.as_ref());
}

fn render_splash(app: &App, area: Rect, buf: &mut Buffer) {
    use crate::ui::visual::{logo, mascot, motif, slot, Decoration};
    use ratatui::widgets::Clear;

    let t = &app.theme;
    // 啟動畫面是**專屬的 overlay**，不是儀表板上的裝飾 —— 它出現的時候
    // 沒有任何監控資料要讓位。所以這裡不看 `app.deco()`：那個等級是
    // 「這台終端機還剩多少餘裕給裝飾」，拿它來降級啟動畫面的結果是
    // 窄畫面上只剩下 logo 幾個字。使用者關掉裝飾時，這個視窗根本不會
    // 被建立（見 `App::new`），所以走到這裡就代表可以完整呈現。
    let deco = Decoration::Full;
    let foot = 1u16; // 版本 / 提示

    // 可用的內容空間：框最多 `area.width - 2` 寬，再扣框線 2 與左右內縮 4，
    // 內容實際只有 `area.width - 8`。少扣兩欄的話，窄終端機上挑出來的
    // 吉祥物會比內容區寬，被截成「…」。高度同理：框 -2、框線 -2、內縮 -2。
    let max_w = area.width.saturating_sub(8);
    let max_h = area.height.saturating_sub(6);

    // logo 放不放得下點陣版，決定它佔幾列
    let pixel_w = logo::pixel_width(app.brand.word()) as u16;
    let brand_lines = if pixel_w <= max_w && max_h >= 8 { 4 } else { 1 };
    let head = brand_lines + 2; // logo + 標語 + 分隔線

    // 吉祥物露幾列，由**寬與高各自能容納多少**決定，不是一道「夠不夠寬」
    // 的閘。窄而高的終端機一樣看得到牠探出頭來 —— 早期版本在這裡卡了
    // 「至少要 66 欄」，於是窄畫面上什麼都沒有。
    // `auto` parse 不出物種，但它不是「不要吉祥物」—— 那是 `none`。
    // 這裡跟其他地方一樣：auto 就是狐狸。
    let species = match app.config.branding.mascot.as_str() {
        "none" => None,
        other => Some(mascot::Species::parse(other).unwrap_or(mascot::Species::Fox)),
    };
    let mut art: Option<mascot::Reveal> = None;
    if let Some(sp) = species {
        let full = mascot::pose(sp, mascot::State::Observe).art.len() as u16 / 2;
        for rows in (4..=full).rev() {
            let r = mascot::reveal(sp, mascot::State::Observe, app.anim_frame, rows);
            if r.w <= max_w && head + r.h + foot <= max_h {
                art = Some(r);
                break;
            }
        }
    }

    // 框照內容算：寬度取 logo 與吉祥物的較大者，高度就是實際要放的東西。
    let art_w = art.as_ref().map_or(0, |a| a.w);
    let art_h = art.as_ref().map_or(0, |a| a.h);
    // +6 = 框線 2 + 左右內縮 4。少算的話點陣 logo 會差一欄而降級成文字。
    let win_w = (pixel_w + 6)
        .max(art_w + 6)
        .max(40)
        .min(area.width.saturating_sub(2));
    let win_h = (head + art_h + foot + 4).min(area.height.saturating_sub(2));
    let win = layout::centered(area, win_w, win_h);
    Clear.render(win, buf);
    crate::ui::widgets::modal::blank_straddling(buf, win);
    // 跟 modal::frame 一樣：自己塗底色，不留給終端機（白底終端機上會是白框）
    buf.set_style(win, t.surface_style());
    let inner = crate::ui::widgets::panel::Panel::new(t, "")
        .border_style(t.style(t.palette.accent))
        .render(win, buf);
    let inner = crate::ui::widgets::panel::inset(inner, 2, 1);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut y = inner.y;
    // 品牌。`slot::logo` 照這裡留給它的高度決定畫法 ——
    // 留了四列就畫得出完整點陣，不會被壓成半高。
    let used = slot::logo(
        buf,
        Rect {
            height: brand_lines.min(inner.height),
            ..inner
        },
        t,
        &app.brand,
        deco,
    );
    y += used.max(1);

    if y < inner.y + inner.height {
        buf.set_string(
            inner.x,
            y,
            format::truncate(logo::slogan(&app.brand), inner.width as usize),
            t.dim_style(),
        );
        y += 1;
    }
    if y < inner.y + inner.height {
        buf.set_string(
            inner.x,
            y,
            // 不帶 ▏ 起頭：視窗的框線就在旁邊，再加一根實心的棒子只會像
            // 多出來的白格（使用者回報過）。
            motif::rule_plain(inner.width as usize),
            t.border_style(),
        );
        y += 1;
    }

    // 吉祥物：置中畫出露出來的那幾列
    if let Some(a) = art {
        let x0 = inner.x + inner.width.saturating_sub(a.w) / 2;
        let ink = t.style(t.palette.fg);
        for (i, line) in a.lines.iter().enumerate() {
            let ly = y + i as u16;
            if ly + foot >= inner.y + inner.height {
                break;
            }
            buf.set_string(x0, ly, format::truncate(line, inner.width as usize), ink);
        }
        if let Some(flag) = Some(app.mascot_flag()) {
            flag.set(true);
        }
    }

    // 版本靠左、開始提示靠右 —— 提示的位置與樣式跟其他 overlay 一致
    let last = inner.y + inner.height - 1;
    buf.set_string(
        inner.x,
        last,
        format::truncate(&format!("sysview {}", crate::VERSION), inner.width as usize),
        t.faint_style(),
    );
    crate::ui::widgets::modal::render_hint(
        buf,
        t,
        inner,
        &crate::ui::widgets::modal::hint(&[crate::ui::widgets::modal::Cap::StartAny]),
    );
}

fn render_focus_bar(app: &App, area: Rect, buf: &mut Buffer) {
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;

    let t = &app.theme;
    let key = t.bold(t.palette.accent);
    let dim = t.dim_style();
    let faint = t.faint_style();
    let sel = t.bold(t.palette.fg).add_modifier(Modifier::REVERSED);

    let mut spans: Vec<Span> = Vec::new();
    match app.focused_region() {
        // 分頁層：還沒進入頁面
        None => {
            spans.push(Span::styled(" ← →", key));
            spans.push(Span::styled(" 切換分頁    ", dim));
            spans.push(Span::styled("↓ / Enter", key));
            spans.push(Span::styled(" 進入這一頁", dim));
        }
        Some(_) => {
            spans.push(Span::styled(" ← ↑ ↓ →", key));
            spans.push(Span::styled(" 移動  ", dim));
            for (i, seg) in app.focus_path().iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" › ", faint));
                }
                spans.push(Span::styled(format!(" {seg} "), sel));
            }
            let (pos, total) = app.focus_position();
            spans.push(Span::styled(format!("  {pos}/{total}"), faint));
            spans.push(Span::styled(
                format!("  第 {} 層", app.focus_depth()),
                faint,
            ));
            if app.can_descend() {
                spans.push(Span::styled("  Enter", key));
                spans.push(Span::styled(" 進入", dim));
            }
            spans.push(Span::styled("  Esc", key));
            spans.push(Span::styled(" 返回", dim));
        }
    }
    Paragraph::new(Line::from(spans)).render(area, buf);

    // 右側的動作提示。用獨立的右對齊段落，不跟左邊搶同一個游標。
    let action = if app.focus.is_some() {
        "e  解釋這一格 "
    } else {
        "0-6 直接跳頁 "
    };
    let w = format::width(action) as u16;
    if w + 4 < area.width {
        let right = Rect {
            x: area.x + area.width - w,
            width: w,
            ..area
        };
        Paragraph::new(Line::from(Span::styled(action, key))).render(right, buf);
    }
}

/// 把焦點框畫在選中的區域上。
///
/// 這是「看到什麼就選什麼」的關鍵：框直接落在畫面上那一塊，
/// 不用再自己對照名詞。
fn render_focus_outline(app: &App, buf: &mut Buffer) {
    let Some(f) = app.focused_region() else {
        return;
    };
    let r = f.rect;
    if r.width < 2 || r.height < 1 {
        return;
    }
    let style = app.theme.bold(app.theme.palette.accent);
    let area = *buf.area();
    let inside = |x: u16, y: u16| x < area.width && y < area.height;

    // 只重畫框線的字元，不動內容
    let (x0, y0) = (r.x, r.y);
    let (x1, y1) = (r.x + r.width - 1, r.y + r.height - 1);

    // 一列高的區域（面板裡的一列、分頁列上的一個分頁）沒有自己的框線，
    // 在上面畫框就等於蓋掉字 —— "Logical CPUs" 會變成 "L▣gical CPUs"，
    // 核心編號會整個不見。這種區域只上色，記號放到旁邊的**空白格**；
    // 找不到空白格就不放記號，顏色已經夠指出是哪一列了。
    if r.height == 1 {
        for x in x0..=x1 {
            if inside(x, y0) {
                if let Some(c) = buf.cell_mut((x, y0)) {
                    c.set_style(style);
                }
            }
        }
        for x in [x0.checked_sub(1), Some(x0)].into_iter().flatten() {
            if !inside(x, y0) {
                continue;
            }
            let blank = buf
                .cell((x, y0))
                .is_some_and(|c| c.symbol().trim().is_empty());
            if blank {
                if let Some(c) = buf.cell_mut((x, y0)) {
                    c.set_char('▸').set_style(style);
                }
                break;
            }
        }
        return;
    }
    for x in x0..=x1 {
        for y in [y0, y1] {
            if inside(x, y) {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.set_style(style);
                }
            }
        }
    }
    for y in y0..=y1 {
        for x in [x0, x1] {
            if inside(x, y) {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.set_style(style);
                }
            }
        }
    }
    // 記號放在圓角「右邊那一格」（本來是 ─），不要蓋掉 ╭，
    // 否則框線的完整性檢查會抓到左右上角數量不一致。
    if x0 + 1 < x1 && inside(x0 + 1, y0) {
        if let Some(c) = buf.cell_mut((x0 + 1, y0)) {
            c.set_char('▣').set_style(style);
        }
    }
}

fn render_footer(app: &App, area: Rect, buf: &mut Buffer) {
    let t = &app.theme;
    // 篩選輸入中
    if app.filter_editing {
        buf.set_string(
            area.x + 1,
            area.y,
            format::truncate(
                &format!("篩選: {}█  (Enter 確定, Esc 取消)", app.filter),
                area.width.saturating_sub(2) as usize,
            ),
            t.bold(t.palette.warning),
        );
        return;
    }
    // 狀態訊息優先
    if let Some(msg) = app.status_text() {
        buf.set_string(
            area.x + 1,
            area.y,
            format::truncate(msg, area.width.saturating_sub(2) as usize),
            t.bold(t.palette.warning),
        );
        return;
    }

    let keys: &[(&str, &str)] = match app.view {
        View::Process => &[
            ("s", "排序"),
            ("/", "篩選"),
            ("↑↓", "捲動"),
            ("e", "解釋"),
            ("0", "總覽"),
            ("?", "說明"),
            ("q", "離開"),
        ],
        View::Admin => &[
            ("[]", "分頁"),
            ("↑↓", "選擇"),
            ("Enter", "明細"),
            ("r", "重查"),
            ("u", "解鎖"),
            ("?", "說明"),
            ("q", "離開"),
        ],
        // ↑↓ 與 e 已經在焦點列上寫得很清楚了，這裡不重複，
        // 把空間讓給沒有其他地方顯示的按鍵。
        View::Cpu => &[
            ("v", "熱度圖"),
            ("A", "管理員"),
            ("空白", "暫停"),
            ("t", "主題"),
            ("m", "吉祥物"),
            ("?", "說明"),
            ("q", "離開"),
        ],
        _ => &[
            ("1-6", "深入"),
            ("A", "管理員"),
            ("空白", "暫停"),
            ("+/-", "更新率"),
            ("t", "主題"),
            ("m", "吉祥物"),
            ("?", "說明"),
            ("q", "離開"),
        ],
    };
    let mut x = area.x + 1;
    for (k, d) in keys {
        let need = format::width(k) as u16 + format::width(d) as u16 + 3;
        if x + need + 26 > area.x + area.width {
            break;
        }
        buf.set_string(x, area.y, *k, t.bold(t.palette.accent));
        x += format::width(k) as u16 + 1;
        buf.set_string(x, area.y, *d, t.dim_style());
        x += format::width(d) as u16 + 2;
    }

    // 右側：權限狀態 + 取樣狀態
    let priv_txt = match app.privilege.state() {
        PrivilegeState::Available => "admin: unlocked",
        PrivilegeState::Locked => "admin: locked (u)",
        PrivilegeState::Unauthorized(_) => "admin: not authorized",
        PrivilegeState::HelperMissing(_) => "admin: helper missing",
    };
    let state = format!(
        "{}  {} {:.1}s",
        priv_txt,
        if app.paused { "‖" } else { "▶" },
        app.interval.as_secs_f64()
    );
    let sx = area.x + area.width.saturating_sub(format::width(&state) as u16 + 1);
    if sx > x {
        buf.set_string(
            sx,
            area.y,
            &state,
            if app.paused {
                t.bold(t.palette.warning)
            } else {
                t.faint_style()
            },
        );
    }
}

fn render_message(app: &App, area: Rect, buf: &mut Buffer, title: &str, body: &str) {
    use crate::ui::widgets::modal::{self, Cap};
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Wrap};
    let t = &app.theme;
    let win = layout::centered(area, 66, 12);
    let inner = modal::frame(
        buf,
        t,
        win,
        &modal::Modal {
            title,
            subtitle: None,
            border: Some(t.palette.warning),
            // 原本這個視窗**什麼提示都沒有** —— 使用者不知道怎麼關掉它
            caps: &[Cap::CloseAny],
        },
    );
    let lines: Vec<Line> = body.lines().map(|l| Line::from(l.to_owned())).collect();
    Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .render(inner, buf);
}

pub fn hostname() -> String {
    crate::collectors::util::read_trimmed("/proc/sys/kernel/hostname")
        .unwrap_or_else(|_| "localhost".into())
}
pub fn kernel_release() -> String {
    crate::collectors::util::read_trimmed("/proc/sys/kernel/osrelease")
        .unwrap_or_else(|_| "unknown".into())
}
pub fn os_name() -> String {
    crate::collectors::util::read_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_owned())
        })
        .unwrap_or_else(|| "Linux".into())
}
fn clock_string() -> String {
    // 不引入 chrono：用 libc 的 localtime_r 就夠了，少一個相依
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: now 是合法的 time_t，tm 是正確大小的輸出緩衝區
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// 從 buffer 讀回某一列的文字。
///
/// 全形字在 ratatui 的 buffer 裡佔兩格：第一格放字，第二格被重設成空白。
/// 直接把所有 symbol 串起來會在每個中文字後面多一個空格，
/// 所以這裡要跳過那些「跟在全形字後面的填充格」。測試與 PTY 驗證都會用到。
pub fn buffer_row(buf: &ratatui::buffer::Buffer, y: u16) -> String {
    use unicode_width::UnicodeWidthStr;
    let area = buf.area();
    let mut out = String::new();
    let mut skip_next = false;
    for x in area.x..area.x + area.width {
        let Some(cell) = buf.cell((x, y)) else {
            continue;
        };
        let s = cell.symbol();
        if skip_next {
            // 寬字元後面那一格**一定**跳過，不管裡面是什麼。
            //
            // 原本只在它是空白時才跳，於是上一幀留在那裡的字會被印出來，
            // 看起來像畫面疊字（`進E入c`）。但那一格終端機根本不會顯示 ——
            // `Buffer::diff` 也是這樣跳過它的，所以它永遠不會被送出去。
            // 這個工具是測試與截圖的眼睛，它說謊比畫面出錯還難查。
            skip_next = false;
            continue;
        }
        if UnicodeWidthStr::width(s) == 2 {
            skip_next = true;
        }
        out.push_str(s);
    }
    out
}

/// 整個畫面的文字內容，一列一行。PTY 測試與快照比對用。
pub fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area();
    (area.y..area.y + area.height)
        .map(|y| buffer_row(buf, y).trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}
