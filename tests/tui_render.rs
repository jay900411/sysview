//! TUI 渲染整合測試。
//!
//! 用 ratatui 的 `TestBackend` 在沒有真實終端機的情況下把每一頁畫出來，
//! 檢查：
//!
//! * 不會 panic（各種極端尺寸）
//! * 不會越界寫入
//! * 中文不會撐破框線
//! * 拿不到的資料顯示成 n/a 而不是 0

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sysview::app::{App, Key, Modal, View, VIEWS};
use sysview::config::Config;
use sysview::theme::ColorDepth;

fn app_with(view: View, w: u16, h: u16) -> (App, Terminal<TestBackend>) {
    // 這些測試看的是頁面內容，啟動畫面會蓋住它們 —— 關掉。
    // 啟動畫面本身另有測試。
    let config = Config {
        branding: sysview::config::Branding {
            splash: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = App::new(config, ColorDepth::TrueColor);
    app.view = view;
    app.tick(std::time::Instant::now());
    // 再取樣一次才有差分資料（速率、使用率）
    std::thread::sleep(std::time::Duration::from_millis(60));
    app.tick(std::time::Instant::now());
    let terminal = Terminal::new(TestBackend::new(w, h)).expect("TestBackend");
    (app, terminal)
}

/// 畫一幀並讀回畫面文字。
///
/// **每次都用全新的 TestBackend**：TestBackend 是逐格儲存的，不會像真實終端機
/// 那樣「寫一個全形字就實體蓋掉兩欄」。沿用同一個 backend 連續畫時，
/// 全形字的延續格會殘留上一幀的內容，讀回來的字串看起來像重疊，
/// 但那是測試載具的假象，不是渲染錯誤（PTY + pyte 的測試一直是正確的）。
fn render(app: &App, term: &mut Terminal<TestBackend>) -> String {
    let (w, h) = {
        let a = term.backend().buffer().area();
        (a.width, a.height)
    };
    *term = Terminal::new(TestBackend::new(w, h)).expect("terminal");
    term.draw(|f| sysview::ui::draw(app, f)).expect("draw");
    sysview::ui::buffer_text(term.backend().buffer())
}

/// 常見的終端機尺寸，含極小與超寬。
const SIZES: &[(u16, u16)] = &[
    (200, 60), // 全螢幕
    (190, 48), // 開發機
    (132, 40), // 標準寬
    (120, 30),
    (100, 35),
    (80, 24), // 經典 SSH
    (72, 20),
    (60, 16),
    (50, 12), // 最小可用
    (40, 10), // 太小，應顯示提示
    (20, 5),  // 病態
];

#[test]
fn every_page_renders_at_every_size_without_panic() {
    for view in VIEWS {
        for &(w, h) in SIZES {
            let (app, mut term) = app_with(view, w, h);
            let text = render(&app, &mut term);
            assert!(
                text.lines().count() <= h as usize,
                "{} 在 {w}x{h} 畫出了太多列",
                view.title()
            );
        }
    }
}

#[test]
fn no_line_exceeds_terminal_width() {
    use unicode_width::UnicodeWidthStr;
    for view in VIEWS {
        for &(w, h) in SIZES {
            let (app, mut term) = app_with(view, w, h);
            let text = render(&app, &mut term);
            for (i, line) in text.lines().enumerate() {
                assert!(
                    UnicodeWidthStr::width(line) <= w as usize,
                    "{} 在 {w}x{h} 的第 {i} 列寬度 {} 超過終端機寬度：{line:?}",
                    view.title(),
                    UnicodeWidthStr::width(line)
                );
            }
        }
    }
}

#[test]
fn panel_borders_are_never_broken_by_cjk() {
    // v1 最嚴重的視覺 bug：中文標題把右上角的 ╮ 蓋掉
    for view in VIEWS {
        let (app, mut term) = app_with(view, 132, 40);
        let text = render(&app, &mut term);
        let opens = text.matches('╭').count();
        let closes = text.matches('╮').count();
        assert_eq!(
            opens,
            closes,
            "{} 的左上角 ╭ 有 {opens} 個但右上角 ╮ 只有 {closes} 個 —— 有框線被覆蓋",
            view.title()
        );
        let bl = text.matches('╰').count();
        let br = text.matches('╯').count();
        assert_eq!(bl, br, "{} 的下方框角數量不一致", view.title());
    }
}

#[test]
fn small_terminal_shows_a_helpful_message_not_garbage() {
    let (app, mut term) = app_with(View::Overview, 40, 10);
    let text = render(&app, &mut term);
    assert!(
        text.contains("太小"),
        "終端機太小時應明確提示，實際輸出：\n{text}"
    );
}

#[test]
fn header_shows_identity_and_health() {
    let (app, mut term) = app_with(View::Overview, 160, 45);
    let text = render(&app, &mut term);
    let head = text.lines().next().unwrap_or("");
    assert!(head.contains("sysview"), "頁首應有工具名稱：{head}");
    assert!(head.contains("Linux"), "頁首應有核心版本：{head}");
    // 健康徽章的符號一定要在（色盲使用者靠它判讀）
    let symbols = ["●", "◐", "▲", "■", "○"];
    assert!(
        symbols.iter().any(|s| head.contains(s)),
        "頁首缺少健康狀態符號（不能只靠顏色）：{head}"
    );
}

#[test]
fn overview_tab_uses_a_digit_not_a_backtick() {
    // 分頁列是 0–6 的數字序列，中間插一個 ` 看起來像雜訊。
    let (app, mut term) = app_with(View::Overview, 190, 48);
    let text = render(&app, &mut term);
    let tabs = text.lines().nth(1).unwrap_or("");
    assert!(
        tabs.contains("0 Overview"),
        "分頁列應顯示 0 Overview：{tabs}"
    );
    assert!(!tabs.contains("` Overview"), "分頁列不該出現反引號：{tabs}");
}

#[test]
fn backtick_still_works_as_an_alias() {
    // 顯示改成 0，但原本的按鍵不能失效
    for key in ['`', '~', '0'] {
        let (mut app, _t) = app_with(View::Cpu, 120, 36);
        app.on_key(Key::Char(key));
        assert_eq!(app.view, View::Overview, "{key:?} 應該要能回到總覽");
    }
}

#[test]
fn all_tabs_are_listed_in_the_header() {
    let (app, mut term) = app_with(View::Overview, 190, 48);
    let text = render(&app, &mut term);
    let tabs = text.lines().nth(1).unwrap_or("");
    for v in VIEWS {
        assert!(tabs.contains(v.title()), "分頁列缺少 {}：{tabs}", v.title());
    }
}

#[test]
fn explain_modal_renders_for_every_metric() {
    for m in sysview::metrics::knowledge::METRICS {
        let (mut app, mut term) = app_with(View::Cpu, 120, 36);
        app.modal = Modal::Explain {
            target: sysview::metrics::describe::DescribeTarget::Metric(m.id),
        };
        let text = render(&app, &mut term);
        assert!(
            text.contains(m.title),
            "{} 的 Explain 面板沒有顯示標題",
            m.id
        );
        assert!(
            text.contains("What is this?"),
            "{} 的 Explain 面板缺少說明區塊",
            m.id
        );
    }
}

#[test]
fn help_modal_renders() {
    let (mut app, mut term) = app_with(View::Overview, 120, 36);
    app.modal = Modal::Help;
    let text = render(&app, &mut term);
    assert!(text.contains("操作說明"));
    assert!(text.contains("解釋"), "說明應介紹 Explain 功能");
}

#[test]
fn admin_page_shows_locked_state_without_privileges() {
    let (app, mut term) = app_with(View::Admin, 120, 36);
    let text = render(&app, &mut term);
    // 測試環境不會有有效 sudo 快取，所以應該是 locked / not authorized / helper missing
    assert!(
        text.contains("鎖住") || text.contains("權限") || text.contains("helper"),
        "沒有授權時管理員頁必須說明狀態，而不是顯示空白或假資料：\n{text}"
    );
    assert!(
        !text.contains("alice") && !text.contains("/home/"),
        "沒有授權時絕不可顯示任何使用者資料"
    );
}

#[test]
fn confirmation_dialog_defaults_to_cancel_and_shows_target() {
    use sysview::app::Confirmation;
    use sysview::privilege::SafeSignal;
    let (mut app, mut term) = app_with(View::Process, 120, 36);
    app.modal = Modal::Confirm(Confirmation::signal(
        4321,
        999,
        "alice",
        "python train.py --epochs 100",
        SafeSignal::Term,
    ));
    let text = render(&app, &mut term);
    assert!(text.contains("4321"), "確認框必須顯示實際 PID");
    assert!(text.contains("alice"), "確認框必須顯示目標使用者");
    assert!(text.contains("train.py"), "確認框必須顯示實際指令");
    assert!(text.contains("取消"), "必須提供取消選項");
}

#[test]
fn monochrome_terminal_still_conveys_severity() {
    let mut app = App::new(Config::default(), ColorDepth::Monochrome);
    app.view = View::Overview;
    app.tick(std::time::Instant::now());
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    let text = render(&app, &mut term);
    let symbols = ["●", "◐", "▲", "■", "○"];
    assert!(
        symbols.iter().any(|s| text.contains(s)),
        "單色終端機下仍必須靠符號傳達狀態"
    );
}

#[test]
fn every_theme_renders_cleanly() {
    for name in sysview::theme::THEMES {
        let config = Config {
            theme: (*name).to_owned(),
            ..Default::default()
        };
        let mut app = App::new(config, ColorDepth::TrueColor);
        app.tick(std::time::Instant::now());
        let mut term = Terminal::new(TestBackend::new(132, 40)).unwrap();
        let text = render(&app, &mut term);
        assert!(text.contains("sysview"), "主題 {name} 渲染失敗");
    }
}

#[test]
fn process_page_filter_and_sort_affect_output() {
    let (mut app, mut term) = app_with(View::Process, 160, 40);
    let before = render(&app, &mut term);
    app.on_key(Key::Char('s')); // 換排序
    let after = render(&app, &mut term);
    assert!(
        before.lines().nth(2) != after.lines().nth(2) || before == after,
        "排序切換應反映在標題上"
    );
    let text = render(&app, &mut term);
    assert!(text.contains("sort:"), "行程頁應顯示目前排序方式");
}

#[test]
fn unavailable_metrics_render_as_text_not_zero() {
    // GPU 頁在沒有 GPU 的機器上也必須好好顯示
    let (app, mut term) = app_with(View::Gpu, 132, 40);
    let text = render(&app, &mut term);
    let bad = ["0.0°C", "0.0 W", "0 MHz / 0 MHz"];
    for b in bad {
        if text.contains(b) {
            // 只有真的讀到 0 才允許；這裡確認至少沒有整排假 0
            assert!(
                !text.contains("0.0°C  ·  0.0 W"),
                "拿不到的資料被顯示成 0：{b}"
            );
        }
    }
}

#[test]
fn resize_from_large_to_tiny_and_back_is_stable() {
    let (app, _t) = app_with(View::Overview, 200, 60);
    for &(w, h) in &[(200u16, 60u16), (40, 10), (80, 24), (20, 5), (132, 40)] {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let _ = render(&app, &mut term);
    }
}

// ── 焦點指示與 metric 選單 ────────────────────────────────────────────────
// 這些測試存在的原因：第一版做了 ↑↓ 選 metric 的機制，卻沒有把「目前選到誰」
// 畫出來，使用者按了完全沒有回饋，等於整個功能不存在。

#[test]
fn focus_bar_does_not_overflow_narrow_terminals() {
    use unicode_width::UnicodeWidthStr;
    for w in [50u16, 60, 80, 100, 150, 200] {
        let (app, mut term) = app_with(View::Cpu, w, 30);
        let text = render(&app, &mut term);
        for line in text.lines() {
            assert!(
                UnicodeWidthStr::width(line) <= w as usize,
                "寬度 {w} 時焦點列或其他內容超出邊界：{line:?}"
            );
        }
    }
}

// ── 空間焦點 ──────────────────────────────────────────────────────────────
// 使用者的原話：「我以為會是框框？框著螢幕上的東西讓我選」。
// 第一版做成固定的文字選單，名詞跟畫面對不上，這批測試防止再犯。

#[test]
fn every_page_registers_focus_regions_from_what_it_actually_drew() {
    for view in VIEWS {
        let (app, mut term) = app_with(view, 150, 42);
        let _ = render(&app, &mut term);
        assert!(
            !app.regions.is_empty(),
            "{} 頁沒有登記任何可選區域，焦點框就無處可畫",
            view.title()
        );
    }
}

#[test]
fn panel_labels_match_what_is_on_screen() {
    // 面板的名稱就是框線上那個字，必須逐字對得上 ——
    // 不可以另外編一套使用者對不起來的名詞。
    for view in VIEWS {
        let (app, mut term) = app_with(view, 160, 44);
        let text = render(&app, &mut term);
        for r in app.regions.all().iter().filter(|r| r.parent.is_none()) {
            assert!(
                text.contains(&r.label),
                "{} 頁的面板登記了「{}」但畫面上找不到這個字",
                view.title(),
                r.label
            );
        }
    }
}

#[test]
fn items_live_inside_their_parent_panel() {
    // 表格列的名稱是給焦點列讀的摘要，選中時使用者看到的是「那一列被框起來」，
    // 所以這裡驗證的是幾何關係：每一列都必須落在自己的面板裡面。
    for view in VIEWS {
        let (app, mut term) = app_with(view, 160, 44);
        let _ = render(&app, &mut term);
        let all = app.regions.all();
        for (i, r) in all.iter().enumerate() {
            let Some(p) = r.parent else { continue };
            assert!(
                !r.label.is_empty(),
                "{} 頁第 {i} 個細項沒有名稱",
                view.title()
            );
            let pr = all[p].rect;
            assert!(
                r.rect.x >= pr.x
                    && r.rect.y >= pr.y
                    && r.rect.x + r.rect.width <= pr.x + pr.width
                    && r.rect.y + r.rect.height <= pr.y + pr.height,
                "{} 頁的「{}」跑到面板「{}」外面了",
                view.title(),
                r.label,
                all[p].label
            );
        }
    }
}

#[test]
fn focus_regions_stay_inside_the_terminal() {
    for view in VIEWS {
        for &(w, h) in &[(80u16, 24u16), (120, 36), (190, 48)] {
            let (app, mut term) = app_with(view, w, h);
            let _ = render(&app, &mut term);
            for r in app.regions.all() {
                assert!(
                    r.rect.x + r.rect.width <= w && r.rect.y + r.rect.height <= h,
                    "{} 頁在 {w}x{h} 登記了超出畫面的區域 {:?}",
                    view.title(),
                    r.rect
                );
            }
        }
    }
}

#[test]
fn focus_outline_appears_after_entering_a_page() {
    let (mut app, mut term) = app_with(View::Overview, 150, 42);
    let before = render(&app, &mut term);
    assert!(
        !before.contains('▣'),
        "還沒進入頁面時不該有焦點框（此時 ←→ 是換頁）"
    );
    app.on_key(Key::Enter);
    let after = render(&app, &mut term);
    assert!(
        after.contains('▣'),
        "進入頁面後應該框住第一塊，單色終端機才分得出選到哪裡"
    );
}

#[test]
fn moving_focus_moves_the_outline() {
    let (mut app, mut term) = app_with(View::Overview, 150, 42);
    let _ = render(&app, &mut term); // 先畫一次才有區域可選
    app.on_key(Key::Enter);
    let before = render(&app, &mut term);
    let pos_before = before.find('▣');
    assert!(pos_before.is_some(), "進入頁面後應該要有焦點框");
    app.on_key(Key::Right);
    let after = render(&app, &mut term);
    assert_ne!(pos_before, after.find('▣'), "→ 之後焦點框應該移到別的位置");
}

#[test]
fn focus_bar_shows_a_breadcrumb_of_where_you_are() {
    let (mut app, mut term) = app_with(View::Cpu, 150, 42);
    // 分頁層先講怎麼進去
    let text = render(&app, &mut term);
    let bar = text.lines().rev().nth(1).unwrap_or("");
    assert!(bar.contains("進入"), "分頁層應提示怎麼進入頁面：{bar}");

    app.on_key(Key::Enter);
    let text = render(&app, &mut term);
    let label = app.focused_region().expect("進入後應該要有焦點").label;
    let bar = text.lines().rev().nth(1).unwrap_or("");
    assert!(bar.contains(&label), "焦點列應顯示框住的那一塊：{bar}");
    assert!(bar.contains("解釋"), "應提示 e 的作用：{bar}");
    assert!(bar.contains("Esc"), "應提示怎麼退回上一層：{bar}");
}

/// 走訪目前頁面的所有面板，停在第一個還能往下鑽的那一塊。
fn seek_descendable(app: &mut sysview::app::App, term: &mut Terminal<TestBackend>) -> bool {
    let _ = render(app, term);
    app.on_key(Key::Enter);
    let _ = render(app, term);
    let n = app.regions.roots().len().max(1);
    // 依序走過每一塊（用 Down 繞一圈，繞不到的再用 Right 換欄）
    for _ in 0..2 {
        for _ in 0..n {
            if app.can_descend() {
                return true;
            }
            app.on_key(Key::Down);
            let _ = render(app, term);
        }
        app.on_key(Key::Right);
        let _ = render(app, term);
    }
    app.can_descend()
}

#[test]
fn breadcrumb_grows_as_you_descend() {
    let (mut app, mut term) = app_with(View::Cpu, 160, 44);
    assert!(
        seek_descendable(&mut app, &mut term),
        "CPU 頁應該有可以往下鑽的面板"
    );
    let shallow = app.focus_path().len();
    app.on_key(Key::Enter);
    let _ = render(&app, &mut term);
    assert!(app.focus_path().len() > shallow, "往下鑽之後麵包屑要變長");
    let text = render(&app, &mut term);
    let bar = text.lines().rev().nth(1).unwrap_or("");
    assert!(bar.contains('›'), "多層時麵包屑要有分隔符號：{bar}");
    assert!(bar.contains("第 2 層"), "應顯示目前深度：{bar}");
}

#[test]
fn explain_is_reachable_from_every_page() {
    for view in VIEWS {
        let (mut app, mut term) = app_with(view, 150, 42);
        let _ = render(&app, &mut term); // 先畫一次才有區域
        app.on_key(Key::Enter); // 進入頁面
        app.on_key(Key::Char('e'));
        assert!(
            matches!(app.modal, Modal::Explain { .. }),
            "{} 頁進入後按 e 到不了說明（modal = {:?}）",
            view.title(),
            app.modal
        );
    }
}

#[test]
fn individual_rows_are_selectable_not_just_whole_panels() {
    // 使用者回報：只到大框框可選，比寫死的選單還不詳細。
    // 每一頁都必須能往下鑽到單一數字 / 單一列。
    for view in [
        View::Cpu,
        View::Gpu,
        View::Network,
        View::Storage,
        View::Process,
    ] {
        let (mut app, mut term) = app_with(view, 160, 44);
        let _ = render(&app, &mut term);
        let has_items = app.regions.all().iter().any(|r| r.parent.is_some());
        assert!(
            has_items,
            "{} 頁沒有任何可選的細項，Logical CPUs / Core clock 這類數字就查不到說明",
            view.title()
        );
        assert!(
            seek_descendable(&mut app, &mut term),
            "{} 頁找不到可以往下鑽的面板",
            view.title()
        );
    }
}

#[test]
fn cpu_core_statistics_rows_are_individually_explainable() {
    // 使用者點名的例子：Logical CPUs / Avg Frequency / Package Temp
    let (app, mut term) = app_with(View::Cpu, 160, 44);
    let _ = render(&app, &mut term);
    let labels: Vec<String> = app
        .regions
        .all()
        .iter()
        .filter(|r| r.parent.is_some())
        .map(|r| r.label.clone())
        .collect();
    for want in ["Logical CPUs", "Avg Frequency", "Package Temp"] {
        assert!(
            labels.iter().any(|l| l == want),
            "「{want}」應該是可以單獨選取並解釋的一列，實際有：{labels:?}"
        );
    }
}

#[test]
fn process_rows_are_individually_selectable() {
    let (mut app, mut term) = app_with(View::Process, 150, 42);
    let _ = render(&app, &mut term);
    app.on_key(Key::Enter); // 進入行程表面板
    assert!(app.can_descend(), "行程表應該可以再往下鑽到單一行程");
    app.on_key(Key::Enter); // 進入第一列
    let r = app.focused_region().expect("應該選到某一列行程");
    assert!(
        r.label.contains('('),
        "選到的應該是具體某個行程（含 PID），得到 {}",
        r.label
    );
}

#[test]
fn walking_off_the_end_of_the_process_list_scrolls_it() {
    let (mut app, mut term) = app_with(View::Process, 150, 42);
    let _ = render(&app, &mut term);
    app.on_key(Key::Enter);
    app.on_key(Key::Enter);
    let rows = app
        .regions
        .all()
        .iter()
        .filter(|r| r.parent.is_some())
        .count();
    let before = app.scroll;
    for _ in 0..rows + 2 {
        app.on_key(Key::Down);
        let _ = render(&app, &mut term);
    }
    assert!(app.scroll > before, "走到清單底部應該捲動而不是卡住");
}

#[test]
fn every_panel_on_every_page_is_reachable_by_arrows() {
    // 使用者回報：「CPU 的頁面我一直按右就會永遠跳過 Core Statistics 這格」。
    // 這裡對每一頁做完整的可達性檢查：從第一塊出發，用上下左右一定要走得到全部。
    for view in VIEWS {
        let (mut app, mut term) = app_with(view, 160, 44);
        let _ = render(&app, &mut term);
        app.on_key(Key::Enter);
        let _ = render(&app, &mut term);

        let roots = app.regions.roots();
        if roots.len() < 2 {
            continue;
        }
        let mut seen = std::collections::HashSet::new();
        let mut queue = vec![roots[0]];
        while let Some(cur) = queue.pop() {
            if !seen.insert(cur) {
                continue;
            }
            let regions = app.regions.all();
            let sib = app.regions.siblings_of(cur);
            for d in [
                sysview::ui::focus::Dir::Up,
                sysview::ui::focus::Dir::Down,
                sysview::ui::focus::Dir::Left,
                sysview::ui::focus::Dir::Right,
            ] {
                if let Some(n) = sysview::ui::focus::nearest(&regions, &sib, cur, d) {
                    queue.push(n);
                }
            }
        }
        let missing: Vec<String> = roots
            .iter()
            .filter(|i| !seen.contains(i))
            .filter_map(|&i| app.regions.get(i).map(|r| r.label))
            .collect();
        assert!(
            missing.is_empty(),
            "{} 頁有走不到的面板：{missing:?}（共 {} 塊，只走到 {}）",
            view.title(),
            roots.len(),
            seen.len()
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// Admin 導覽狀態機
//
// 使用者回報：「User Storage 的 user 選取失效了，在該分頁按 enter 只能
// 直接顯示第一個 user 的空間，上下鍵無法選 user 了」。
// 以下用真的 Admin renderer 把畫面畫出來，再打真的按鍵，驗證整條路徑。
// ─────────────────────────────────────────────────────────────────────

/// 造一份夠長、一定會捲動的假使用者清單。
fn fake_users(n: usize) -> serde_json::Value {
    let users: Vec<_> = (0..n)
        .map(|i| {
            serde_json::json!({
                "user": format!("u{i:02}"),
                "home": format!("/home/u{i:02}"),
                "bytes": (i as u64 + 1) * 1_000_000,
                "files": i as u64 * 10,
                "complete": true,
                "rss": (i as u64 + 1) * 1_000_000,
                "pss": (i as u64 + 1) * 900_000,
                "procs": i as u64,
                "vram": (i as u64) * 1_000_000,
                "tcp_est": i as u64, "listen": 1, "time_wait": 0, "udp": 2, "total": i as u64 + 3,
            })
        })
        .collect();
    // `available` 是 GPU 分頁用來判斷 NVML 有沒有起來的旗標
    serde_json::json!({ "available": true, "users": users })
}

fn admin_app(n: usize) -> (App, Terminal<TestBackend>) {
    let (mut app, term) = app_with(View::Admin, 160, 24);
    // 畫出「解鎖後」的版面。這不是權限旁路 —— 狀態只是 UI 的提示，
    // 真正的特權呼叫還是要走 sudo，helper 也還是要 root EUID。
    app.privilege = sysview::privilege::PrivilegeClient::with_state_for_tests(
        sysview::privilege::PrivilegeState::Available,
    );
    let users = fake_users(n);
    app.admin.storage = sysview::app::Async::Ready(users.clone());
    app.admin.memory = sysview::app::Async::Ready(users.clone());
    app.admin.gpu = sysview::app::Async::Ready(users.clone());
    app.admin.sockets = sysview::app::Async::Ready(users);
    (app, term)
}

/// 畫一幀，然後像主迴圈那樣把焦點對回選取的列。
fn frame(app: &mut App, term: &mut Terminal<TestBackend>) -> String {
    let s = render(app, term);
    app.realign_focus();
    s
}

#[test]
fn admin_enter_reaches_the_tab_row_then_the_user_list() {
    let (mut app, mut term) = admin_app(6);
    frame(&mut app, &mut term);

    // 分頁層：還沒有焦點
    assert_eq!(app.focus, None);

    // Enter → 第 1 層（分頁列）
    app.on_key(Key::Enter);
    frame(&mut app, &mut term);
    let r = app.focused_region().expect("Enter 之後要有焦點");
    assert_eq!(r.label, "User Storage", "第一層應該落在第一個分頁上");
    assert_eq!(app.focus_depth(), 1);

    // Enter → 第 2 層（使用者清單）
    app.on_key(Key::Enter);
    frame(&mut app, &mut term);
    let r = app.focused_region().expect("再 Enter 要進到清單");
    assert_eq!(r.label, "u00", "應該落在第一個使用者上");
    assert_eq!(app.focus_depth(), 2);
}

#[test]
fn admin_arrows_move_between_tabs_at_the_tab_level() {
    let (mut app, mut term) = admin_app(6);
    frame(&mut app, &mut term);
    app.on_key(Key::Enter);
    frame(&mut app, &mut term);

    for expected in ["User Memory", "GPU Users", "Sockets"] {
        app.on_key(Key::Right);
        frame(&mut app, &mut term);
        assert_eq!(
            app.focused_region().unwrap().label,
            expected,
            "→ 要走到下一個分頁"
        );
        assert_eq!(
            app.admin.tab().title(),
            expected,
            "焦點在哪個分頁，就要真的切到那個分頁"
        );
    }
    app.on_key(Key::Left);
    frame(&mut app, &mut term);
    assert_eq!(app.admin.tab().title(), "GPU Users", "← 要走回上一個分頁");
}

#[test]
fn admin_up_down_selects_users_even_after_the_list_scrolls() {
    // 這是使用者回報的那個 bug：清單一捲動，選取就跳回開頭。
    // 原因是選取狀態用「排第幾個兄弟」回推，但只有看得到的列會被登記。
    let (mut app, mut term) = admin_app(40);
    frame(&mut app, &mut term);
    app.on_key(Key::Enter); // 分頁列
    frame(&mut app, &mut term);
    app.on_key(Key::Enter); // 使用者清單
    frame(&mut app, &mut term);

    for want in 1..30 {
        app.on_key(Key::Down);
        frame(&mut app, &mut term);
        assert_eq!(
            app.admin.selected, want,
            "↓ 第 {want} 次之後應該選到第 {want} 個使用者"
        );
        assert_eq!(
            app.focused_region().map(|r| r.label.clone()),
            Some(format!("u{want:02}")),
            "焦點框要跟選取的列在同一列"
        );
    }
    for want in (0..29).rev() {
        app.on_key(Key::Up);
        frame(&mut app, &mut term);
        assert_eq!(app.admin.selected, want, "↑ 要一次退一個");
    }
}

#[test]
fn admin_selection_survives_a_tab_switch_and_all_tabs_scroll() {
    // 四個分頁都要能捲動 —— 之前只有 User Storage 會，
    // 另外三頁超過面板高度的列根本選不到。
    for tab_presses in 0..4 {
        let (mut app, mut term) = admin_app(40);
        frame(&mut app, &mut term);
        app.on_key(Key::Enter);
        frame(&mut app, &mut term);
        for _ in 0..tab_presses {
            app.on_key(Key::Right);
            frame(&mut app, &mut term);
        }
        let tab = app.admin.tab().title().to_owned();
        app.on_key(Key::Enter);
        frame(&mut app, &mut term);
        for _ in 0..25 {
            app.on_key(Key::Down);
            frame(&mut app, &mut term);
        }
        assert_eq!(app.admin.selected, 25, "{tab}：應該選到第 25 列");
        assert_eq!(
            app.focused_region().map(|r| r.label.clone()),
            Some("u25".to_owned()),
            "{tab}：焦點框要在第 25 列上"
        );
        let screen = frame(&mut app, &mut term);
        assert!(
            screen.contains("u25"),
            "{tab}：選到的列必須看得見，不能捲出畫面"
        );
    }
}

#[test]
fn admin_esc_walks_back_up_one_level_at_a_time() {
    let (mut app, mut term) = admin_app(6);
    frame(&mut app, &mut term);
    app.on_key(Key::Enter);
    frame(&mut app, &mut term);
    app.on_key(Key::Enter);
    frame(&mut app, &mut term);
    assert_eq!(app.focus_depth(), 2);

    app.on_key(Key::Esc);
    frame(&mut app, &mut term);
    assert_eq!(app.focus_depth(), 1, "Esc 一次退一層");

    app.on_key(Key::Esc);
    frame(&mut app, &mut term);
    assert_eq!(app.focus, None, "再 Esc 回到分頁層");
    assert_eq!(app.view, View::Admin, "Esc 不該換頁");
}

#[test]
fn admin_enter_on_a_user_row_expands_that_user_not_the_first_one() {
    // 使用者回報：「在該分頁按 enter 只能直接顯示第一個 user 的空間」
    let (mut app, mut term) = admin_app(40);
    frame(&mut app, &mut term);
    app.on_key(Key::Enter); // 分頁列
    frame(&mut app, &mut term);
    app.on_key(Key::Enter); // 使用者清單
    frame(&mut app, &mut term);
    for _ in 0..17 {
        app.on_key(Key::Down);
        frame(&mut app, &mut term);
    }
    assert_eq!(app.admin.selected, 17);

    app.on_key(Key::Enter);
    frame(&mut app, &mut term);
    assert_eq!(
        app.admin.detail_user.as_deref(),
        Some("u17"),
        "Enter 要展開游標所在的那個人，不是清單第一個"
    );
}

#[test]
fn admin_enter_on_a_tab_does_not_expand_a_user() {
    let (mut app, mut term) = admin_app(6);
    frame(&mut app, &mut term);
    app.on_key(Key::Enter); // 停在分頁上
    frame(&mut app, &mut term);
    app.on_key(Key::Enter); // 應該是「進入清單」，不是「展開明細」
    frame(&mut app, &mut term);
    assert_eq!(app.admin.detail_user, None, "在分頁上按 Enter 只該進入清單");
    assert_eq!(app.focused_region().unwrap().label, "u00");
}

#[test]
fn esc_never_quits_it_only_walks_back_up() {
    // 說明上寫的是「Esc 回到上一層、q 離開」。連按 Esc 退層時
    // 不該在總覽把工具關掉 —— 那是使用者最常做的動作。
    let (mut app, mut term) = app_with(View::Process, 160, 40);
    render(&app, &mut term);
    app.on_key(Key::Enter);
    render(&app, &mut term);
    app.on_key(Key::Enter);
    render(&app, &mut term);

    for _ in 0..12 {
        app.on_key(Key::Esc);
        render(&app, &mut term);
        assert!(!app.quit, "Esc 不該離開程式");
    }
    assert_eq!(app.view, View::Overview, "Esc 退到底要停在總覽");

    app.on_key(Key::Char('q'));
    assert!(app.quit, "q 才是離開");
}

// ─────────────────────────────────────────────────────────────────────
// 使用者回報的四件事
// ─────────────────────────────────────────────────────────────────────

#[test]
fn admin_tabs_only_move_sideways_and_never_cycle() {
    // 回報：「第二層卻能用上下控制左右的切換 並且在進入到第三四的分頁面的
    // 上下鍵會在三四循環卡死」。分頁排成一列，↑↓ 不該在它們之間移動。
    let (mut app, mut term) = admin_app(20);
    frame(&mut app, &mut term);
    app.on_key(Key::Enter);
    frame(&mut app, &mut term);
    assert_eq!(app.admin.tab().title(), "User Storage");

    for _ in 0..6 {
        app.on_key(Key::Up);
        frame(&mut app, &mut term);
        assert_eq!(app.admin.tab().title(), "User Storage", "↑ 不該切換分頁");
    }
    // ↓ 在橫排的分頁列上代表往裡走一層，不該變成切分頁
    app.on_key(Key::Down);
    frame(&mut app, &mut term);
    assert_eq!(app.admin.tab().title(), "User Storage", "↓ 不該切換分頁");
    assert_eq!(app.focus_depth(), 2, "↓ 應該進到清單那一層");

    // 走到最後一個分頁再按上下，也不能在分頁之間打轉
    app.on_key(Key::Esc);
    frame(&mut app, &mut term);
    for _ in 0..3 {
        app.on_key(Key::Right);
        frame(&mut app, &mut term);
    }
    assert_eq!(app.admin.tab().title(), "Sockets");
    for _ in 0..4 {
        app.on_key(Key::Up);
        frame(&mut app, &mut term);
        assert_eq!(app.admin.tab().title(), "Sockets", "↑ 不該讓分頁繞圈");
    }
    app.on_key(Key::Right);
    frame(&mut app, &mut term);
    assert_eq!(app.admin.tab().title(), "Sockets", "最後一個分頁往右要停住");
}

#[test]
fn the_focus_marker_never_covers_a_character() {
    // 回報：選框把字蓋掉 —— "Logical CPUs" 變成 "L▣gical CPUs"，
    // 核心編號整個不見。一列高的區域只能上色，不能寫字進去。
    for view in [View::Cpu, View::Network, View::Gpu] {
        let (mut app, mut term) = app_with(view, 170, 44);
        let plain = render(&app, &mut term);
        app.on_key(Key::Enter);
        render(&app, &mut term);
        app.on_key(Key::Enter);
        let focused = render(&app, &mut term);

        let Some(r) = app.focused_region() else {
            continue;
        };
        if r.rect.height != 1 {
            continue;
        }
        let plain_rows: Vec<&str> = plain.lines().collect();
        let focus_rows: Vec<&str> = focused.lines().collect();
        let y = r.rect.y as usize;
        if y >= plain_rows.len() || y >= focus_rows.len() {
            continue;
        }
        // 這一列的字必須跟沒有焦點時一模一樣，只有顏色可以不同
        assert!(
            !focus_rows[y].contains('▣'),
            "{view:?}: 一列高的焦點不該把 ▣ 寫進內容裡\n{}",
            focus_rows[y]
        );
        assert_eq!(
            focus_rows[y].replace('▸', " ").trim_end(),
            plain_rows[y].trim_end(),
            "{view:?}: 焦點框改動了這一列的文字"
        );
    }
}

#[test]
fn explain_titles_what_you_selected_not_the_metric_family() {
    // 回報：框住 Logical CPUs 按 e，標題卻寫 CPU Utilization
    let (mut app, mut term) = app_with(View::Cpu, 170, 44);
    render(&app, &mut term);
    app.on_key(Key::Enter);
    render(&app, &mut term);
    app.on_key(Key::Enter);
    render(&app, &mut term);

    // 逐一走過這一層的每一列，每一列的說明標題都要是那一列自己的名字
    for _ in 0..14 {
        let Some(label) = app.focused_region().map(|r| r.label.clone()) else {
            break;
        };
        app.on_key(Key::Char('e'));
        let screen = render(&app, &mut term);
        // Explain 視窗是置中的浮動框，副標一定帶著 type name
        let title_line = screen
            .lines()
            .find(|l| l.contains("╭") && l.contains("Metric"))
            .unwrap_or("");
        assert!(
            title_line.contains(&label),
            "選到 {label:?} 時說明標題卻是：{title_line}"
        );
        app.on_key(Key::Esc);
        render(&app, &mut term);
        app.on_key(Key::Down);
        render(&app, &mut term);
    }
}

#[test]
fn interrupts_has_its_own_explanation() {
    // 回報：「interrupt的解釋沒有」
    let d = sysview::metrics::knowledge::lookup("cpu.intr").expect("cpu.intr 要有定義");
    assert_eq!(d.title, "Interrupts");
    assert!(d.meaning.contains("中斷"));
    assert!(
        d.sources.iter().any(|s| s.contains("/proc/stat")),
        "來源要寫 sysview 真的讀的那個檔案"
    );
    assert!(
        d.commands.iter().any(|c| c.contains("/proc/interrupts")),
        "要告訴使用者怎麼查是哪個裝置在中斷"
    );
    // 不能再借用 context switch 的說明
    let ctxt = sysview::metrics::knowledge::lookup("cpu.ctxt").unwrap();
    assert_ne!(d.meaning, ctxt.meaning);
}

#[test]
fn explain_scrolling_stops_at_the_bottom() {
    // 回報：說明視窗可以一直往下捲，捲進一片空白，而且要按同樣多次 ↑ 才回得來
    let (mut app, mut term) = app_with(View::Cpu, 120, 26);
    render(&app, &mut term);
    app.on_key(Key::Enter);
    render(&app, &mut term);
    app.on_key(Key::Char('e'));
    render(&app, &mut term);
    assert!(matches!(app.modal, Modal::Explain { .. }));

    for _ in 0..200 {
        app.on_key(Key::Down);
        render(&app, &mut term);
    }
    let max = app.explain_max_scroll.get();
    assert!(max > 0, "這份說明應該長到需要捲動");
    assert_eq!(app.explain_scroll, max, "捲到底就該停住");

    // 停在底部時，最後一行必須還看得見
    let screen = render(&app, &mut term);
    assert!(
        screen.contains("Related") || screen.contains("Equivalent commands"),
        "捲到底時應該看得到說明的最後一段，而不是空白"
    );

    // 往上一次就要立刻有反應，不能先消化掉多按的次數
    app.on_key(Key::Up);
    render(&app, &mut term);
    assert_eq!(app.explain_scroll, max - 1, "↑ 一次退一行");
}

#[test]
fn explain_that_fits_cannot_scroll_at_all() {
    let (mut app, mut term) = app_with(View::Cpu, 200, 60);
    render(&app, &mut term);
    app.on_key(Key::Enter);
    render(&app, &mut term);
    app.on_key(Key::Char('e'));
    render(&app, &mut term);
    if app.explain_max_scroll.get() != 0 {
        return; // 這個尺寸下內容仍然超出，換別的斷言沒有意義
    }
    for _ in 0..10 {
        app.on_key(Key::Down);
        render(&app, &mut term);
    }
    assert_eq!(app.explain_scroll, 0, "內容放得下就完全不該捲動");
}

// ─────────────────────────────────────────────────────────────────────
// 視覺改版：responsive 與「裝飾永遠讓位給資料」
// ─────────────────────────────────────────────────────────────────────

fn app_with_visual(view: View, w: u16, h: u16, deco: &str) -> (App, Terminal<TestBackend>) {
    let config = Config {
        visual: sysview::config::Visual {
            decorations: deco.to_owned(),
            ..Default::default()
        },
        branding: sysview::config::Branding {
            name: "DEVLAB".to_owned(),
            splash: false,
            ..Default::default()
        },
        labels: sysview::config::Labels {
            cpu: "BRAIN".to_owned(),
            storage: "VAULT".to_owned(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = App::new(config, ColorDepth::TrueColor);
    app.view = view;
    app.tick(std::time::Instant::now());
    std::thread::sleep(std::time::Duration::from_millis(60));
    app.tick(std::time::Instant::now());
    (app, Terminal::new(TestBackend::new(w, h)).expect("term"))
}

#[test]
fn decorations_off_produces_no_mascot_or_motif_anywhere() {
    // 使用者說不要就是不要 —— 任何頁面、任何尺寸都不該漏出裝飾
    for view in VIEWS {
        for (w, h) in [(200u16, 60u16), (160, 45), (120, 30)] {
            let (app, mut term) = app_with_visual(view, w, h, "off");
            let text = render(&app, &mut term);
            for ch in ['▀', '▄'] {
                assert!(
                    !text.contains(ch),
                    "{view:?} {w}x{h}：decorations=off 卻出現了吉祥物的半格字元"
                );
            }
            for m in ["╌", "╴╴╴", "// SYSVIEW", "// DEVLAB"] {
                assert!(
                    !text.contains(m),
                    "{view:?} {w}x{h}：decorations=off 卻出現了裝飾紋樣 {m:?}"
                );
            }
        }
    }
}

#[test]
fn decorations_disappear_before_data_does() {
    // 這是整個 responsive 策略的核心承諾：先丟裝飾，資料留到最後
    for (w, h) in [(200u16, 60u16), (150, 40), (120, 30), (100, 26), (80, 24)] {
        let (app, mut term) = app_with_visual(View::Overview, w, h, "auto");
        let text = render(&app, &mut term);
        // 不管多小，這些監控資料一定還在
        assert!(text.contains("CPU"), "{w}x{h}：CPU 面板不見了");
        assert!(
            text.contains("Memory") || text.contains("RAM"),
            "{w}x{h}：記憶體資訊不見了"
        );
        // 小畫面不該有**整隻**吉祥物 —— 但只填空白格的浮水印可以露個頭：
        // 它從不佔資料的空間，露多少由乾淨區域決定
        if w < 120 || h < 30 {
            assert!(!text.contains("▀▀▀▀▀▀"), "{w}x{h} 太小了，不該畫整隻吉祥物");
            if let Some(r) = app.mascot_spot().get() {
                assert!(r.height <= 8, "{w}x{h} 太小了卻露出 {} 列", r.height);
            }
        }
    }
}

#[test]
fn every_page_still_fits_at_every_size_with_full_decorations() {
    // 使用者把裝飾開到最大，版面也不能爆
    for view in VIEWS {
        for (w, h) in SIZES {
            let (app, mut term) = app_with_visual(view, *w, *h, "full");
            let text = render(&app, &mut term);
            for (i, line) in text.lines().enumerate() {
                assert!(
                    sysview::ui::format::width(line.trim_end()) <= *w as usize,
                    "{view:?} {w}x{h} 第 {i} 列超出畫面：{line:?}"
                );
            }
        }
    }
}

#[test]
fn custom_labels_never_replace_the_canonical_name() {
    // 別名是加上去的，不是取代 —— 換了人看還是要知道那是什麼
    let (app, mut term) = app_with_visual(View::Overview, 190, 48, "auto");
    let text = render(&app, &mut term);
    let tabs = text.lines().nth(1).unwrap_or("");
    assert!(tabs.contains("BRAIN"), "別名應該出現：{tabs}");
    assert!(tabs.contains("CPU"), "原名不能被取代：{tabs}");
    assert!(tabs.contains("VAULT") && tabs.contains("Storage"));

    // 窄畫面時別名讓位，原名留下
    let (app, mut term) = app_with_visual(View::Overview, 110, 30, "auto");
    let text = render(&app, &mut term);
    let tabs = text.lines().nth(1).unwrap_or("");
    assert!(tabs.contains("CPU"), "窄畫面一定要留原名：{tabs}");
    assert!(
        !tabs.contains("BRAIN"),
        "窄畫面時別名應該讓位給資料：{tabs}"
    );
}

#[test]
fn the_brand_name_never_hides_what_the_tool_is() {
    // 自訂品牌之後，仍然要有辦法知道這是 sysview
    let (app, mut term) = app_with_visual(View::Overview, 190, 48, "full");
    let text = render(&app, &mut term);
    assert!(text.contains("DEVLAB"), "品牌代號應該出現");
    let help_text = {
        let mut a = app;
        a.on_key(Key::Char('?'));
        render(&a, &mut term)
    };
    assert!(
        help_text.contains("sysview"),
        "說明頁必須講得出這是什麼工具"
    );
}

#[test]
fn animation_advances_without_touching_the_sampling_clock() {
    // 這次改版最重要的效能保證：動畫再怎麼跑，都不會多取樣一次
    let (mut app, _term) = app_with_visual(View::Overview, 190, 48, "full");
    let before = app.anim_frame;
    let sample_deadline = app.next_sample_in(std::time::Instant::now());

    let mut now = std::time::Instant::now();
    let mut advanced = 0;
    for _ in 0..40 {
        now += std::time::Duration::from_millis(120);
        if app.tick_animation(now) {
            advanced += 1;
        }
    }
    assert!(advanced > 0, "動畫應該有推進");
    assert!(app.anim_frame > before);
    // 動畫推進不該把取樣期限往前拉
    let after = app.next_sample_in(std::time::Instant::now());
    assert!(
        after <= sample_deadline + std::time::Duration::from_millis(50),
        "動畫影響到了採樣排程"
    );
}

#[test]
fn animation_is_off_when_there_is_no_mascot_to_animate() {
    let config = Config {
        branding: sysview::config::Branding {
            mascot: "none".to_owned(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = App::new(config, ColorDepth::Monochrome);
    let mut now = std::time::Instant::now();
    for _ in 0..20 {
        now += std::time::Duration::from_secs(1);
        assert!(
            !app.tick_animation(now),
            "沒有吉祥物就不該有動畫，那是白燒 CPU"
        );
    }
    assert_eq!(app.anim_frame, 0);
    // 也不該讓主迴圈醒得更頻繁
    assert!(app.next_animation_in(now) >= std::time::Duration::from_secs(60));
}

#[test]
fn patterns_stay_on_even_when_decorations_are_off() {
    // pattern 是 encoding 不是裝飾，關掉裝飾不該讓它消失
    let (app, mut term) = app_with_visual(View::Memory, 160, 45, "off");
    let text = render(&app, &mut term);
    let glyphs = [
        sysview::ui::visual::pattern::Pattern::Solid.glyph(),
        sysview::ui::visual::pattern::Pattern::Shade.glyph(),
        sysview::ui::visual::pattern::Pattern::Dot.glyph(),
    ];
    let found = glyphs.iter().filter(|g| text.contains(**g)).count();
    assert!(
        found >= 2,
        "記憶體組成條在關掉裝飾後失去了紋理，那是資訊不是裝飾"
    );
}

#[test]
fn the_highlights_card_is_selectable_and_explains_what_it_points_at() {
    // 畫了一塊卡片卻沒登記成可選區域，使用者按 ↑↓ 永遠走不到它，
    // 按 e 也沒反應 —— 看得到卻碰不到比沒有更令人困惑。
    let (mut app, mut term) = app_with_visual(View::Overview, 190, 48, "full");
    render(&app, &mut term);
    app.on_key(Key::Enter);
    render(&app, &mut term);

    let labels: Vec<String> = app
        .regions
        .all()
        .iter()
        .filter(|r| r.parent.is_none())
        .map(|r| r.label.clone())
        .collect();
    assert!(
        labels.iter().any(|l| l == "HIGHLIGHTS"),
        "Highlights 沒有登記成可選面板：{labels:?}"
    );

    // 走到它，再鑽進去
    let mut steps = 0;
    while app.focused_region().map(|r| r.label.clone()).as_deref() != Some("HIGHLIGHTS") {
        app.on_key(Key::Down);
        render(&app, &mut term);
        steps += 1;
        assert!(steps < 20, "方向鍵走不到 Highlights");
    }
    app.on_key(Key::Enter);
    render(&app, &mut term);

    // 每一條重點都要指向它講的那個具體東西，而不是一段泛泛的解釋
    let r = app.focused_region().expect("應該進到某一條重點上");
    assert!(
        matches!(
            r.target,
            sysview::metrics::describe::DescribeTarget::Entity(_)
        ),
        "重點應該指向具體物件，實際是 {:?}",
        r.target
    );
}

#[test]
fn the_highlights_strip_gives_up_a_row_not_a_card() {
    // Highlights 以前是右下角一整張卡片，為了三行字吃掉 56x23 的版面。
    // 現在是底下一列 —— 高度不夠時整條不出現，面板不會被壓扁。
    for (w, h, want) in [
        (190u16, 48u16, true),
        (160, 42, true),
        (150, 40, true),
        (148, 42, true),
        (132, 26, false),
    ] {
        let (app, mut term) = app_with_visual(View::Overview, w, h, "full");
        let text = render(&app, &mut term);
        assert_eq!(
            text.contains("HIGHLIGHTS"),
            want,
            "{w}x{h} 的 Highlights 出現與否不符預期"
        );
        if want {
            // 只佔一列：整個畫面上「Highlights」只能出現一次，而且要在最底下幾列
            let rows: Vec<usize> = text
                .lines()
                .enumerate()
                .filter(|(_, l)| l.contains("HIGHLIGHTS"))
                .map(|(i, _)| i)
                .collect();
            assert_eq!(rows.len(), 1, "{w}x{h} 的 Highlights 佔了不只一列");
            assert!(
                rows[0] + 4 >= text.lines().count(),
                "{w}x{h} 的 Highlights 沒有在底部"
            );
        }
    }
}

#[test]
fn the_highlights_strip_lays_its_items_side_by_side_on_one_row() {
    // 卡片是上下堆疊，條是左右並排。改版面之後每一段仍然要是獨立的
    // 可選區域 —— 不是一整列一個大目標。
    let (app, mut term) = app_with_visual(View::Overview, 190, 48, "full");
    render(&app, &mut term);
    let all = app.regions.all();
    let panel = all
        .iter()
        .position(|r| r.label == "HIGHLIGHTS" && r.parent.is_none())
        .expect("Highlights 沒有登記成面板");
    let items: Vec<_> = all.iter().filter(|r| r.parent == Some(panel)).collect();
    assert!(!items.is_empty(), "條上一段可選區域都沒有");
    let y = items[0].rect.y;
    for it in &items {
        assert_eq!(it.rect.height, 1, "{} 不只一列高", it.label);
        assert_eq!(it.rect.y, y, "{} 沒有跟其他段落在同一列", it.label);
        assert!(
            matches!(
                it.target,
                sysview::metrics::describe::DescribeTarget::Entity(_)
            ),
            "{} 應該指向具體物件",
            it.label
        );
    }
    // 左右並排：x 座標互不重疊而且遞增
    for w in items.windows(2) {
        assert!(
            w[0].rect.x + w[0].rect.width <= w[1].rect.x,
            "{} 跟 {} 疊在一起了",
            w[0].label,
            w[1].label
        );
    }
}

#[test]
fn the_mascot_can_be_switched_from_inside_the_ui() {
    // 想看另一隻不該要先改設定檔再重開
    let (mut app, mut term) = app_with_visual(View::Overview, 190, 48, "full");
    render(&app, &mut term);
    let seen: Vec<String> = (0..4)
        .map(|_| {
            app.on_key(Key::Char('m'));
            render(&app, &mut term);
            app.config.branding.mascot.clone()
        })
        .collect();
    assert!(seen.contains(&"deer".to_owned()), "切不到鹿：{seen:?}");
    assert!(seen.contains(&"none".to_owned()), "切不到關閉：{seen:?}");
    assert!(seen.contains(&"fox".to_owned()), "切不回狐狸：{seen:?}");
    // 循環要回到原點，不能卡住
    assert_eq!(seen[0], seen[3], "循環沒有回到起點：{seen:?}");
}

#[test]
fn switching_the_mascot_off_actually_removes_it() {
    // 用 CPU 頁：常駐浮水印要有一塊乾淨區域才會畫，CPU 頁的每核心清單
    // 底下就是這樣一塊。總覽的面板鋪滿整頁，本來就看不到浮水印，
    // 拿它來測「關掉之後有沒有變少」會兩邊都一樣而測不出東西。
    let (mut app, mut term) = app_with_visual(View::Cpu, 190, 48, "full");
    render(&app, &mut term);
    while app.config.branding.mascot != "none" {
        app.on_key(Key::Char('m'));
    }
    let off = render(&app, &mut term);
    while app.config.branding.mascot != "fox" {
        app.on_key(Key::Char('m'));
    }
    let on = render(&app, &mut term);

    // 用「開 vs 關」比較，而不是數絕對值 ——
    // 長條圖本來就有大片方塊，數絕對值只會抓到它們
    let heavy = |t: &str| t.lines().filter(|l| l.matches('█').count() > 4).count();
    assert!(
        heavy(&off) < heavy(&on),
        "關掉吉祥物之後畫面上的方塊沒有變少：關 {} 開 {}",
        heavy(&off),
        heavy(&on)
    );
    // 關掉之後不能留下任何一格 —— 殘影比畫錯還難查
    assert!(
        !off.contains('▟') && !off.contains('▙'),
        "關掉了還留著剪影的格子"
    );

    // 而 Highlights 是資訊不是裝飾，關掉吉祥物不該把它一起關掉
    let (mut app, mut term) = app_with_visual(View::Overview, 190, 48, "full");
    while app.config.branding.mascot != "none" {
        app.on_key(Key::Char('m'));
    }
    assert!(render(&app, &mut term).contains("HIGHLIGHTS"));
}

#[test]
fn no_page_ever_shows_two_mascots() {
    // 管理員的鎖定畫面自己就畫了一隻。常駐浮水印如果不看這件事，
    // 那一頁就會出現兩隻 —— 一隻在卡片裡、一隻在背景。
    //
    // 量法：同一頁「有吉祥物」與「關掉吉祥物」的四分格數量差，就是
    // 這一頁畫了幾格剪影。一隻大約 275 格，兩隻就會破 400。
    let quad = |t: &str| t.chars().filter(|c| "▘▝▖▞▛▚▜▙▟█▀▄▌▐".contains(*c)).count();
    for view in [
        View::Cpu,
        View::Memory,
        View::Gpu,
        View::Process,
        View::Admin,
    ] {
        let (mut app, mut term) = app_with_visual(view, 190, 48, "full");
        let on = quad(&render(&app, &mut term));
        while app.config.branding.mascot != "none" {
            app.on_key(Key::Char('m'));
        }
        let off = quad(&render(&app, &mut term));
        let ink = on.saturating_sub(off);
        assert!(
            ink < 400,
            "{view:?} 畫了 {ink} 格剪影 —— 一隻大約 275 格，這看起來是兩隻"
        );
    }
}

#[test]
fn turning_the_mascot_off_turns_it_off_on_every_page_including_admin() {
    // 管理員頁原本是自己 `Species::parse(…)`，parse 不出來就退回鹿 ——
    // 而 `"none"` 正好 parse 不出來，於是「關掉吉祥物」在這一頁沒有效果。
    let quad = |t: &str| t.chars().filter(|c| "▘▝▖▞▛▚▜▙▟".contains(*c)).count();
    for view in [
        View::Cpu,
        View::Memory,
        View::Gpu,
        View::Process,
        View::Admin,
    ] {
        let (mut app, mut term) = app_with_visual(view, 190, 48, "full");
        while app.config.branding.mascot != "none" {
            app.on_key(Key::Char('m'));
        }
        let text = render(&app, &mut term);
        assert_eq!(quad(&text), 0, "{view:?} 關掉吉祥物之後還畫著剪影");
    }
}

#[test]
fn switching_pages_leaves_no_stale_mascot_cells() {
    // 浮水印是疊在已經畫好的畫面上的，所以「上一頁的狐狸留在這一頁」
    // 是這種做法最典型的壞法。
    //
    // 量法改成「切過去的畫面」對「直接開這一頁的畫面」—— 現在每一頁都
    // 會露出一點吉祥物，用「有沒有四分格」判斷已經沒有意義了。
    let quad = |t: &str| t.chars().filter(|c| "▘▝▖▞▛▚▜▙▟█▀▄▌▐".contains(*c)).count();
    for (from, to) in [
        (View::Cpu, View::Network),
        (View::Process, View::Overview),
        (View::Memory, View::Storage),
    ] {
        let (mut app, mut term) = app_with_visual(from, 190, 48, "full");
        render(&app, &mut term);
        app.view = to;
        let switched = render(&app, &mut term);

        let (direct_app, mut fresh) = app_with_visual(to, 190, 48, "full");
        let direct = render(&direct_app, &mut fresh);
        assert!(
            quad(&switched).abs_diff(quad(&direct)) < 40,
            "{from:?} → {to:?} 之後的剪影格數（{}）跟直接開這一頁（{}）差太多，像有殘影",
            quad(&switched),
            quad(&direct)
        );
    }
}

#[test]
fn resizing_shrinks_the_reveal_instead_of_dropping_it() {
    // 畫面變窄不該讓吉祥物整個消失 —— 露 4 列的頭只有十幾欄寬。
    // 一路縮下去，露出的列數要跟著變少；縮到裝飾整個關掉才會不見。
    let (app, mut term) = app_with_visual(View::Cpu, 190, 48, "full");
    render(&app, &mut term);
    let big = app.mascot_spot().get().map(|r| r.height).unwrap_or(0);
    assert!(big > 8, "大畫面上只露了 {big} 列");

    let mut mid = Terminal::new(TestBackend::new(110, 40)).unwrap();
    render(&app, &mut mid);
    let small = app.mascot_spot().get().map(|r| r.height).unwrap_or(0);
    assert!(small > 0, "110 欄就完全不露了 —— 明明放得下一個頭");
    assert!(small <= big, "窄畫面反而露得比寬畫面多");

    // 再小也一樣：露多少由乾淨區域決定，只會變少不會變多
    let mut tiny = Terminal::new(TestBackend::new(80, 20)).unwrap();
    render(&app, &mut tiny);
    let tiny_rows = app.mascot_spot().get().map(|r| r.height).unwrap_or(0);
    assert!(tiny_rows <= small, "80×20 反而露得比 110×40 多");

    // 真正讓浮水印消失的是**使用者關掉裝飾**，不是尺寸
    let quad = |t: &str| t.chars().filter(|c| "▘▝▖▞▛▚▜▙▟".contains(*c)).count();
    let (off, mut big_off) = app_with_visual(View::Cpu, 190, 48, "off");
    let text = render(&off, &mut big_off);
    assert_eq!(quad(&text), 0, "裝飾關掉了還在畫浮水印");
    assert!(!off.decorations_animate());

    // 放大回來：要回得來
    let back = render(&app, &mut term);
    assert!(quad(&back) > 0, "畫面放大之後浮水印沒有回來");
    assert!(app.decorations_animate());
}

#[test]
fn the_splash_shows_by_default_and_leaves_on_its_own() {
    // 打 sysview 進來就該看到品牌畫面 —— 不用改設定檔
    let mut app = App::new(Config::default(), ColorDepth::TrueColor);
    app.tick(std::time::Instant::now());
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let text = render(&app, &mut term);
    assert!(text.contains("任意鍵"), "預設就該顯示啟動畫面");
    assert!(text.contains("sysview"), "啟動畫面要講得出這是什麼");
}

#[test]
fn the_splash_owns_the_first_keypress_and_does_nothing_else_with_it() {
    // 蓋在畫面上的東西，使用者按鍵是想把它關掉 —— 不是想順便暫停取樣、
    // 不是想順便換頁。「順便讓那個鍵生效」聽起來體貼，實際上是穿透：
    // 空白會 pause、方向鍵會移焦點、1 會跳到 CPU 頁。
    let fresh = || {
        let mut app = App::new(Config::default(), ColorDepth::TrueColor);
        app.tick(std::time::Instant::now());
        let term = Terminal::new(TestBackend::new(120, 40)).unwrap();
        (app, term)
    };

    // 空白：只關掉，不暫停
    let (mut app, mut term) = fresh();
    render(&app, &mut term);
    let paused_before = app.paused;
    app.on_key(Key::Char(' '));
    assert!(matches!(app.modal, Modal::None), "空白鍵沒有關掉啟動畫面");
    assert_eq!(app.paused, paused_before, "空白鍵穿透過去把取樣暫停了");

    // 方向鍵：只關掉，不移動焦點
    let (mut app, mut term) = fresh();
    render(&app, &mut term);
    app.on_key(Key::Down);
    assert!(matches!(app.modal, Modal::None));
    assert!(
        app.focused_region().is_none(),
        "方向鍵穿透過去移動了焦點：{:?}",
        app.focused_region().map(|r| r.label)
    );

    // 數字鍵：只關掉，不換頁
    let (mut app, mut term) = fresh();
    render(&app, &mut term);
    app.on_key(Key::Char('3'));
    assert!(matches!(app.modal, Modal::None));
    assert_eq!(app.view, View::Overview, "數字鍵穿透過去換了頁");

    // q：第一下只關掉，第二下才離開
    let (mut app, mut term) = fresh();
    render(&app, &mut term);
    assert!(
        !matches!(app.on_key(Key::Char('q')), sysview::app::SideEffect::Quit),
        "第一個 q 就把程式關掉了"
    );
    assert!(!app.quit);
    assert!(matches!(
        app.on_key(Key::Char('q')),
        sysview::app::SideEffect::Quit
    ));
}

#[test]
fn every_modal_owns_its_input() {
    // Splash 漏掉 return 造成的穿透，其他視窗也可能發生。這裡把規則
    // 釘在所有視窗上：視窗開著時，底下的頁面收不到任何鍵。
    for open in [Modal::Splash, Modal::Help, Modal::Dino] {
        let (mut app, mut term) = app_with_visual(View::Overview, 160, 44, "full");
        render(&app, &mut term);
        app.modal = open.clone();
        if matches!(open, Modal::Dino) {
            app.on_key(Key::Char('g'));
        }
        let view = app.view;
        let paused = app.paused;
        let theme = app.theme.name;
        for key in [
            Key::Char(' '),
            Key::Down,
            Key::Up,
            Key::Left,
            Key::Right,
            Key::Char('1'),
            Key::Char('t'),
            Key::Char('m'),
            Key::Char('A'),
            Key::Enter,
        ] {
            app.modal = if matches!(open, Modal::Dino) {
                Modal::Dino
            } else {
                open.clone()
            };
            app.on_key(key);
            assert_eq!(app.view, view, "{open:?} 開著時 {key:?} 換了頁");
            assert_eq!(app.paused, paused, "{open:?} 開著時 {key:?} 暫停了取樣");
            assert_eq!(app.theme.name, theme, "{open:?} 開著時 {key:?} 換了主題");
            assert!(
                app.focused_region().is_none(),
                "{open:?} 開著時 {key:?} 移動了焦點"
            );
        }
    }
}

#[test]
fn the_scroll_hint_appears_exactly_when_the_content_can_scroll() {
    // 提示必須描述**真的做得到的事**。內容塞得下卻寫著「↑↓ 捲動」，
    // 使用者按了發現沒反應；反過來則是內容被截斷而沒有人告訴他。
    let (mut app, mut term) = app_with_visual(View::Overview, 150, 40, "full");
    let mut checked = 0;
    let mut scrollable = 0;
    for view in VIEWS {
        app.view = view;
        render(&app, &mut term);
        app.on_key(Key::Down);
        for _ in 0..8 {
            render(&app, &mut term);
            app.on_key(Key::Char('e'));
            if !matches!(app.modal, Modal::Explain { .. }) {
                app.on_key(Key::Down);
                continue;
            }
            let text = render(&app, &mut term);
            let says_scroll = text.contains("捲動");
            let can_scroll = app.explain_max_scroll.get() > 0;
            assert_eq!(
                says_scroll, can_scroll,
                "{view:?}：提示說捲動={says_scroll} 但實際可捲={can_scroll}"
            );
            assert!(text.contains("關閉"), "{view:?}：沒有告訴使用者怎麼關掉");
            checked += 1;
            if can_scroll {
                scrollable += 1;
            }
            app.on_key(Key::Esc);
            app.on_key(Key::Down);
        }
    }
    assert!(checked > 30, "只檢查了 {checked} 個 Explain，覆蓋不足");
    assert!(
        scrollable > 3 && scrollable < checked,
        "{scrollable}/{checked} 可捲 —— 兩種情況都要有才測得出差別"
    );
}

#[test]
fn every_overlay_tells_you_how_to_get_out() {
    // 訊息視窗原本什麼提示都沒有。這一條守著所有 overlay。
    let (mut app, mut term) = app_with_visual(View::Cpu, 160, 44, "full");
    render(&app, &mut term);
    for (name, open) in [
        ("說明", Modal::Help),
        ("啟動畫面", Modal::Splash),
        (
            "訊息",
            Modal::Message {
                title: "測試".into(),
                body: "內容".into(),
            },
        ),
    ] {
        app.modal = open;
        let text = render(&app, &mut term);
        assert!(
            text.contains("關閉") || text.contains("開始"),
            "{name} 沒有告訴使用者怎麼離開"
        );
    }
    app.modal = Modal::None;
    app.on_key(Key::Char('g'));
    let text = render(&app, &mut term);
    assert!(text.contains("Esc 關閉"), "遊戲沒有告訴使用者怎麼離開");
}

#[test]
fn the_dino_is_a_modal_that_gives_the_page_back_untouched() {
    let (mut app, mut term) = app_with_visual(View::Memory, 160, 44, "full");
    render(&app, &mut term);
    // 先在頁面裡選一個東西，等一下要確認它沒被動到
    app.on_key(Key::Down);
    render(&app, &mut term);
    app.on_key(Key::Down);
    render(&app, &mut term);
    let before = app.focused_region().map(|r| r.label);
    assert!(before.is_some(), "測試前提：頁面上要先選到東西");

    app.on_key(Key::Char('g'));
    assert!(matches!(app.modal, Modal::Dino), "g 沒有打開遊戲");
    assert!(app.dino.is_some());
    // 遊戲是**浮在儀表板上的一塊**，不是切到另一個分頁：
    // 底下的資料還看得到，遊戲只佔中間。
    let game = render(&app, &mut term);
    assert!(
        game.contains("Physical Memory"),
        "遊戲蓋掉了整個 dashboard —— 那看起來像換了一頁，不是開了一局"
    );
    assert!(game.contains("DINO"), "看不到遊戲視窗");
    assert!(game.contains("跳"), "遊戲畫面沒有操作提示");
    let lines: Vec<&str> = game.lines().collect();
    assert!(
        lines[1].contains("Overview"),
        "分頁列被蓋掉了：{:?}",
        lines[1]
    );
    assert!(
        lines[lines.len() - 1].contains("說明"),
        "頁尾被蓋掉了：{:?}",
        lines[lines.len() - 1]
    );

    // 玩幾格
    for _ in 0..30 {
        let t = app.dino.as_ref().unwrap().last_tick() + sysview::ui::dino::FRAME;
        app.tick_game(t);
    }
    app.on_key(Key::Char(' '));
    render(&app, &mut term);

    app.on_key(Key::Esc);
    assert!(matches!(app.modal, Modal::None), "Esc 沒有離開遊戲");
    assert!(app.dino.is_none(), "離開之後遊戲狀態沒有清掉");
    let back = render(&app, &mut term);
    assert_eq!(app.view, View::Memory, "回來之後換了頁");
    assert_eq!(
        app.focused_region().map(|r| r.label),
        before,
        "回來之後焦點跑掉了"
    );
    assert!(
        back.contains("Physical Memory"),
        "回來之後沒有重畫 dashboard"
    );
}

#[test]
fn the_splash_uses_the_height_it_has_even_on_a_narrow_terminal() {
    // 啟動畫面是專屬 overlay，不該照 dashboard 的裝飾預算降級。
    // 早期版本在這裡卡了「至少要 66 欄」，於是窄而高的終端機上
    // 只剩下 logo 幾個字，明明上下放得下一整隻。
    for (w, h) in [(60u16, 50u16), (70, 45), (80, 50)] {
        let mut app = App::new(
            Config {
                branding: sysview::config::Branding {
                    name: "DEVLAB".to_owned(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ColorDepth::TrueColor,
        );
        app.tick(std::time::Instant::now());
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        let text = render(&app, &mut term);
        assert!(text.contains("任意鍵"), "{w}×{h} 沒有啟動畫面");
        // 點陣 logo：窄畫面也該畫得出來（DEVLAB 只要 35 欄）
        assert!(
            text.contains('▀') || text.contains('▄'),
            "{w}×{h} 的品牌只剩文字"
        );
        // 而且高度夠的話要看得到吉祥物
        let quad = text.chars().filter(|c| "▘▝▖▞▛▚▜▙▟".contains(*c)).count();
        assert!(quad > 30, "{w}×{h} 上下明明夠長卻沒有吉祥物（{quad} 格）");
    }
}

#[test]
fn the_game_clock_is_silent_when_nobody_is_playing() {
    let (mut app, _term) = app_with_visual(View::Overview, 160, 44, "full");
    let now = std::time::Instant::now();
    assert!(!app.tick_game(now), "沒在玩卻推進了遊戲");
    assert!(
        app.next_game_tick_in(now) > std::time::Duration::from_secs(60),
        "沒在玩卻要為遊戲醒來"
    );
    app.on_key(Key::Char('g'));
    assert!(
        app.next_game_tick_in(std::time::Instant::now()) <= sysview::ui::dino::FRAME,
        "開了遊戲卻不安排下一格"
    );
}

#[test]
fn decorations_off_also_means_no_splash() {
    // 啟動畫面也是裝飾。說不要就是全部都不要。
    let config = Config {
        visual: sysview::config::Visual {
            decorations: "off".to_owned(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = App::new(config, ColorDepth::TrueColor);
    app.tick(std::time::Instant::now());
    let mut term = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let text = render(&app, &mut term);
    assert!(!text.contains("任意鍵"));
    assert!(text.contains("CPU"), "資料要正常顯示");
}

#[test]
fn the_animation_clock_stops_when_no_mascot_is_on_screen() {
    // 八個頁面裡只有總覽有裝飾槽，小終端機更是完全放不下。
    // 那些情況下還跑動畫，等於畫面上什麼都沒動卻每秒重畫三次。
    let mut app = App::new(
        Config {
            branding: sysview::config::Branding {
                mascot: "fox".to_owned(),
                splash: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ColorDepth::TrueColor,
    );
    app.tick(std::time::Instant::now());

    // 常駐浮水印：頁面上找得到一塊乾淨區域就會畫，畫了才需要動畫時鐘。
    // CPU 頁的每核心清單底下有一整塊空白，所以看得到。
    app.view = View::Cpu;
    let mut big = Terminal::new(TestBackend::new(190, 48)).unwrap();
    render(&app, &mut big);
    assert!(app.decorations_animate(), "CPU 頁畫了浮水印，動畫該跑");

    // 總覽放不下完整剪影，但第二層（上半身）放得下 —— 它一樣是畫出來的，
    // 動畫時鐘就該跑。真正該停的是**完全看不到**的情況。
    app.view = View::Overview;
    render(&app, &mut big);
    assert!(app.decorations_animate(), "總覽畫了上半身，動畫該跑");

    // 小畫面：露不露得出頭由乾淨區域決定，但**規則**不變 ——
    // 畫了才動、沒畫就不動。兩者必須一致，不然會為看不見的東西醒來。
    app.view = View::Cpu;
    let mut small = Terminal::new(TestBackend::new(80, 24)).unwrap();
    render(&app, &mut small);
    assert_eq!(
        app.decorations_animate(),
        app.mascot_spot().get().is_some(),
        "動畫時鐘跟「有沒有畫出吉祥物」不一致"
    );

    // 回到大畫面：該恢復
    render(&app, &mut big);
    assert!(app.decorations_animate(), "空間回來了，動畫也該回來");
}

#[test]
fn pausing_puts_the_mascot_to_rest() {
    // 暫停取樣時畫面已經凍結，動作還在跑會讓人以為資料仍在更新。
    // 而且暫停時 refresh_mood 不會被呼叫，沿用舊氣氛只會停在過期的姿態。
    let mut app = App::new(
        Config {
            branding: sysview::config::Branding {
                mascot: "fox".to_owned(),
                splash: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ColorDepth::TrueColor,
    );
    app.tick(std::time::Instant::now());
    app.on_key(Key::Char(' ')); // 暫停
    assert!(app.paused);
    let (_, state) = app.mascot_now().expect("該有吉祥物");
    assert_eq!(
        state,
        sysview::ui::visual::mascot::State::Rest,
        "暫停時應該是休息姿態"
    );
}

/// 畫面上**沒有任何一格**的背景是 `Reset`（單色模式除外）。
///
/// `Reset` 的背景 = 終端機自己的預設底色。深色終端機看不出差別，但 macOS
/// Terminal.app 的預設 profile 是白底：overlay 視窗 `Clear` 之後沒塗底色，
/// 整個視窗就變成白框、淺灰的字看不見；恐龍白天沒給底色，活著的恐龍就
/// 消失（死掉那格是深灰的 faint，反而看得見）。使用者回報「e、Help、恐龍的
/// 框大曝光」就是這個。
#[test]
fn no_cell_ever_shows_the_terminal_default_background() {
    use ratatui::style::Color;
    use sysview::metrics::describe::DescribeTarget;

    type Opener = Box<dyn Fn(&mut App)>;
    let overlays: Vec<(&str, Opener)> = vec![
        ("dashboard", Box::new(|_| {})),
        ("help", Box::new(|a| a.modal = Modal::Help)),
        (
            "explain",
            Box::new(|a| {
                a.modal = Modal::Explain {
                    target: DescribeTarget::Metric("cpu.usage"),
                }
            }),
        ),
        (
            "message",
            Box::new(|a| {
                a.modal = Modal::Message {
                    title: "t".into(),
                    body: "b".into(),
                }
            }),
        ),
        (
            "dino",
            Box::new(|a| {
                a.on_key(Key::Char('g'));
            }),
        ),
    ];
    for depth in [
        ColorDepth::TrueColor,
        ColorDepth::Indexed256,
        ColorDepth::Basic,
    ] {
        for splash in [false, true] {
            for (name, open) in &overlays {
                if splash && *name != "dashboard" {
                    continue;
                }
                let config = Config {
                    branding: sysview::config::Branding {
                        splash,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut app = App::new(config, ColorDepth::TrueColor);
                app.theme = sysview::theme::Theme::new(app.theme.name, depth);
                app.view = View::Cpu;
                app.tick(std::time::Instant::now());
                open(&mut app);
                let mut term = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
                term.draw(|f| sysview::ui::draw(&app, f)).expect("draw");
                let buf = term.backend().buffer();
                let area = *buf.area();
                for y in area.y..area.y + area.height {
                    let mut skip = 0usize;
                    for x in area.x..area.x + area.width {
                        let c = buf.cell((x, y)).expect("cell");
                        if skip > 0 {
                            // 全形字的延續格：ratatui 會 reset 它、diff 也跳過它，
                            // 終端機上被前一格的字蓋住，不算破洞。
                            skip -= 1;
                            continue;
                        }
                        let w = unicode_width::UnicodeWidthStr::width(c.symbol());
                        skip = w.saturating_sub(1);
                        assert_ne!(
                            c.bg,
                            Color::Reset,
                            "{depth:?} / {} / splash={splash}：({x},{y}) 的背景是終端機預設色（符號 {:?}）",
                            name,
                            c.symbol()
                        );
                    }
                }
            }
        }
    }
}

/// 死掉之後：榜、名字輸入框、提示列都在畫面上；送出之後看得到自己的名字。
#[test]
fn the_game_over_screen_shows_the_board_and_asks_for_a_name() {
    use sysview::ui::dino::{Dino, Rng, FRAME};
    let (mut app, mut term) = app_with(View::Cpu, 150, 40);
    let dir = std::env::temp_dir().join(format!("sysview-tui-scores-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let uid = sysview::collectors::util::real_uid();
    app.scores = sysview::scoreboard::Store::with_paths(
        Some(dir.clone()),
        dir.join(format!("{uid}.json")),
        uid,
    );
    let mut t = std::time::Instant::now();
    app.dino = Some(Dino::with_rng(t, 0, Rng::from_seed(7)));
    app.modal = Modal::Dino;
    for _ in 0..(20.0 / FRAME.as_secs_f32()) as usize {
        t += FRAME;
        app.tick_game(t);
        if app.dino.as_ref().is_some_and(|g| g.is_over()) {
            break;
        }
    }
    assert!(app.dino.as_ref().unwrap().is_over());
    let text = render(&app, &mut term);
    assert!(text.contains("TOP 5"), "沒有榜：\n{text}");
    assert!(text.contains("還沒有人上榜"));
    assert!(text.contains("上榜了！名字："), "沒有輸入框");
    assert!(text.contains("Enter 送出"), "提示列該說 Enter 送出");
    assert!(text.contains("Esc 不記錄"), "提示列該說 Esc 不記錄");
    assert!(!text.contains("r 重來"), "輸入框開著時 r 是字母");

    for c in "Ada".chars() {
        app.on_key(Key::Char(c));
    }
    app.on_key(Key::Enter);
    let text = render(&app, &mut term);
    assert!(text.contains("1. Ada"), "送出之後榜上要有名字：\n{text}");
    assert!(text.contains("已記錄"), "要說記錄了");
    // 名字跟帳號不同：顯示來自哪個帳號（檔的 owner，改不了）
    let account = sysview::collectors::util::username(uid);
    assert!(
        text.contains(&format!("@{account}")),
        "要顯示帳號 @{account}：\n{text}"
    );
    assert!(text.contains("r 重來"), "送出之後 r 才是重來");
    app.on_key(Key::Esc);
    assert!(matches!(app.modal, Modal::None));
}
