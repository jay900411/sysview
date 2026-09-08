//! Describe / Explain 系統的完整性與安全測試。
//!
//! 核心不變量：
//!
//! > UI 裡任何可以被 focus / Enter / Explain 的東西，都必須對應到一份
//! > 合法的說明。不允許出現「No description available」。

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sysview::app::{App, Key, View, VIEWS};
use sysview::collectors::{Intervals, SystemState};
use sysview::config::Config;
use sysview::metrics::describe::{describe, DescribeTarget, EntityRef};
use sysview::metrics::knowledge::METRICS;
use sysview::theme::ColorDepth;

fn state() -> SystemState {
    let mut s = SystemState::with_options(60, Intervals::default(), true);
    s.sample_all(std::time::Instant::now());
    std::thread::sleep(std::time::Duration::from_millis(150));
    s.sample_all(std::time::Instant::now());
    s
}

fn app_rendered(view: View) -> (App, Terminal<TestBackend>) {
    let mut app = App::new(Config::default(), ColorDepth::TrueColor);
    // 啟動畫面是 modal，第一個鍵只用來關它。測試要測的是頁面上的按鍵，
    // 所以先把它關掉，不然每個測試的第一個動作都被吃掉。
    app.modal = sysview::app::Modal::None;
    app.view = view;
    app.tick(std::time::Instant::now());
    std::thread::sleep(std::time::Duration::from_millis(150));
    app.tick(std::time::Instant::now());
    let mut term = Terminal::new(TestBackend::new(170, 46)).unwrap();
    term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
    (app, term)
}

/// 跟 `app_rendered` 一樣，但關掉 GPU collector —— 模擬 CI runner 那種沒有 GPU 的機器。
fn app_rendered_without_gpu(view: View) -> (App, Terminal<TestBackend>) {
    let mut app = App::new(
        Config {
            gpu: false,
            ..Default::default()
        },
        ColorDepth::TrueColor,
    );
    // 啟動畫面是 modal，第一個鍵只用來關它。測試要測的是頁面上的按鍵，
    // 所以先把它關掉，不然每個測試的第一個動作都被吃掉。
    app.modal = sysview::app::Modal::None;
    app.view = view;
    app.tick(std::time::Instant::now());
    std::thread::sleep(std::time::Duration::from_millis(150));
    app.tick(std::time::Instant::now());
    let mut term = Terminal::new(TestBackend::new(170, 46)).unwrap();
    term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
    (app, term)
}

// ── 固定 metric 的完整性 ─────────────────────────────────────────────────

#[test]
fn every_fixed_metric_is_fully_documented() {
    for m in METRICS {
        assert!(!m.title.is_empty(), "{} 缺 title", m.id);
        assert!(m.meaning.len() > 10, "{} 的 meaning 太短", m.id);
        assert!(!m.sources.is_empty(), "{} 沒寫資料來源", m.id);
        assert!(!m.formula.is_empty(), "{} 沒寫算式", m.id);
        assert!(!m.pitfalls.is_empty(), "{} 沒寫 pitfalls", m.id);
        assert!(!m.commands.is_empty(), "{} 沒給原生指令", m.id);
    }
}

#[test]
fn metric_sources_look_like_real_linux_interfaces() {
    // 來源必須指向真的東西：/proc、/sys、libc 函式、或具名的函式庫
    for m in METRICS {
        for s in m.sources {
            let plausible = s.starts_with("/proc")
                || s.starts_with("/sys")
                || s.contains("statvfs")
                || s.contains("getifaddrs")
                || s.contains("NVML")
                || s.contains("amdgpu")
                || s.contains("i915")
                || s.contains("hwmon")
                || s.contains("走訪");
            assert!(plausible, "{} 的來源 {s:?} 看起來不像真的 Linux 介面", m.id);
        }
    }
}

#[test]
fn every_referenced_command_names_a_real_binary_or_is_a_file_read() {
    // 指令必須是這台機器上真的存在的程式，否則就是在誤導使用者。
    // 允許 cat/grep 這種讀檔的寫法，以及 sudo 前綴。
    let mut missing = Vec::new();
    for m in METRICS {
        for cmd in m.commands {
            let first = cmd
                .split_whitespace()
                .find(|w| *w != "sudo" && *w != "watch")
                .unwrap_or("");
            if first.is_empty() || first.starts_with('/') || first.starts_with('$') {
                continue;
            }
            let ok = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("command -v {first} >/dev/null 2>&1"))
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                missing.push(format!("{} → {first}", m.id));
            }
        }
    }
    // 這台機器上不一定裝了全部工具，所以只印出來提醒，不直接失敗；
    // 但核心工具一定要在。
    if !missing.is_empty() {
        eprintln!("這台機器上沒有的指令（不是錯誤，只是提醒）：{missing:#?}");
    }
    for core in ["ps", "df", "free", "uptime", "cat"] {
        let ok = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {core} >/dev/null 2>&1"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "核心工具 {core} 不存在，測試環境有問題");
    }
}

// ── 動態 entity ──────────────────────────────────────────────────────────

#[test]
fn every_entity_kind_produces_a_complete_description_on_this_machine() {
    let s = state();
    let mut targets: Vec<(&str, DescribeTarget)> = vec![
        ("CpuCore", DescribeTarget::Entity(EntityRef::CpuCore(0))),
        (
            "User",
            DescribeTarget::Entity(EntityRef::User(sysview::collectors::util::real_uid())),
        ),
        ("Path", DescribeTarget::Entity(EntityRef::Path("/".into()))),
    ];
    for d in &s.gpu.state().devices {
        targets.push(("Gpu", DescribeTarget::Entity(EntityRef::Gpu(d.id.clone()))));
    }
    for m in &s.disk.state().mounts {
        targets.push((
            "Mount",
            DescribeTarget::Entity(EntityRef::Mount(m.mount_point.clone())),
        ));
    }
    for d in &s.disk.state().devices {
        targets.push((
            "Disk",
            DescribeTarget::Entity(EntityRef::Disk(d.name.clone())),
        ));
    }
    for i in &s.network.state().interfaces {
        targets.push((
            "NetworkInterface",
            DescribeTarget::Entity(EntityRef::NetworkInterface(i.name.clone())),
        ));
    }
    for p in s.process.state().processes.iter().take(3) {
        targets.push((
            "Process",
            DescribeTarget::Entity(EntityRef::Process {
                pid: p.pid,
                starttime: p.starttime,
            }),
        ));
    }

    let mut seen = std::collections::HashSet::new();
    for (kind, t) in &targets {
        seen.insert(*kind);
        let c = describe(t, &s);
        assert!(!c.title.is_empty(), "{t:?} 沒有標題");
        assert!(!c.summary.is_empty(), "{t:?} 沒有 What is this?");
        assert!(!c.current.is_empty(), "{t:?} 沒有任何當前值");
        assert!(!c.source.is_empty(), "{t:?} 沒有標示來源");
        assert!(!c.commands.is_empty(), "{t:?} 沒有給原生指令");
        assert!(!c.type_name.is_empty());
    }
    // 這台機器上至少要驗到這幾種
    for must in [
        "CpuCore",
        "User",
        "Path",
        "Mount",
        "Disk",
        "NetworkInterface",
        "Process",
    ] {
        assert!(seen.contains(must), "沒有驗到 {must} 這種 entity");
    }
}

#[test]
fn describe_does_not_invent_semantics() {
    // 不可以自作聰明說「這是開發專案」「這是在訓練模型」之類的
    let s = state();
    let c = describe(&DescribeTarget::Entity(EntityRef::Path("/home".into())), &s);
    // 檢查的是有沒有做出「肯定的語意宣稱」，
    // 「不推測這個目錄的用途」這種否定句反而是我們要的。
    let text = format!("{} {}", c.summary, c.title);
    for claim in [
        "這是開發專案",
        "應該是",
        "可能是為了",
        "看起來像",
        "用來存放",
    ] {
        assert!(
            !text.contains(claim),
            "路徑說明不該猜語意，出現了 {claim:?}：{text}"
        );
    }
    assert!(
        c.summary.contains("不推測"),
        "應明講不推測用途：{}",
        c.summary
    );
    // 欄位只能是 OS 真的提供的中繼資料
    let labels: Vec<&str> = c.current.iter().map(|f| f.label.as_str()).collect();
    for l in &labels {
        assert!(
            [
                "路徑",
                "型別",
                "擁有者",
                "權限",
                "大小",
                "所在檔案系統",
                "檔案系統類型",
                "中繼資料"
            ]
            .contains(l),
            "路徑說明出現了不該有的欄位：{l}"
        );
    }
}

#[test]
fn gpu_description_reflects_the_backend_actually_used() {
    let s = state();
    for d in &s.gpu.state().devices {
        let c = describe(&DescribeTarget::Entity(EntityRef::Gpu(d.id.clone())), &s);
        assert!(
            c.source.iter().any(|x| x == d.backend),
            "{} 的 Source 應該是實際 backend {:?}，得到 {:?}",
            d.name,
            d.backend,
            c.source
        );
        // Intel 的使用率是推估的，必須標出來
        if d.utilization.quality.is_estimated() {
            let has_estimated = c
                .current
                .iter()
                .any(|f| f.certainty == sysview::metrics::describe::Certainty::Estimated);
            assert!(has_estimated, "{} 的推估值沒有標記", d.name);
            let how = c.how_obtained.clone().unwrap_or_default().to_lowercase();
            assert!(how.contains("rc6"), "推估值必須說明算法，實際說明是：{how}");
        }
    }
}

#[test]
fn vanished_process_is_reported_not_faked() {
    let s = state();
    // 用一個不可能存在的 (pid, starttime) 組合
    let c = describe(
        &DescribeTarget::Entity(EntityRef::Process {
            pid: 999_999,
            starttime: 1,
        }),
        &s,
    );
    assert!(
        c.summary.contains("結束") || c.summary.contains("重用"),
        "行程消失時要明講，而不是編數字：{}",
        c.summary
    );
}

// ── UI 覆蓋率 ────────────────────────────────────────────────────────────

/// 沒有 GPU 的機器（CI runner、一般筆電的容器）GPU 頁只有一句「沒有偵測到
/// GPU」，本來就沒有東西可選 —— 那不是頁面忘了登記區域。有 GPU 的機器才
/// 檢查它。
fn gpu_page_is_empty_here(app: &sysview::app::App, view: View) -> bool {
    view == View::Gpu && app.state.gpu.state().devices.is_empty()
}

#[test]
fn every_selectable_ui_element_has_a_valid_describe_target() {
    for view in VIEWS {
        let (app, _t) = app_rendered(view);
        if gpu_page_is_empty_here(&app, view) {
            continue;
        }
        assert!(
            !app.regions.is_empty(),
            "{} 頁沒有任何可選區域",
            view.title()
        );
        let s = &app.state;
        for r in app.regions.all() {
            let c = describe(&r.target, s);
            assert!(
                !c.is_empty(),
                "{} 頁的「{}」({:?}) 沒有可用的說明",
                view.title(),
                r.label,
                r.target
            );
            assert!(
                !c.summary.is_empty(),
                "{} 頁的「{}」缺 What is this?",
                view.title(),
                r.label
            );
            // 固定 metric 一定要指向知識庫裡真的存在的條目
            if let DescribeTarget::Metric(id) = &r.target {
                assert!(
                    sysview::metrics::lookup(id).is_some(),
                    "{} 頁的「{}」指向不存在的 metric {id}",
                    view.title(),
                    r.label
                );
            }
        }
    }
}

#[test]
fn no_selectable_element_says_no_description_available() {
    for view in VIEWS {
        let (app, _t) = app_rendered(view);
        for r in app.regions.all() {
            let c = describe(&r.target, &app.state);
            let text = format!("{} {}", c.summary, c.title);
            assert!(
                !text.contains("No description"),
                "{} 頁的「{}」出現了無說明的佔位文字",
                view.title(),
                r.label
            );
        }
    }
}

#[test]
fn pressing_e_on_any_selectable_element_opens_a_description() {
    for view in VIEWS {
        let (mut app, mut term) = app_rendered(view);
        if gpu_page_is_empty_here(&app, view) {
            continue;
        }
        app.on_key(Key::Enter);
        term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
        for _ in 0..6 {
            app.on_key(Key::Char('e'));
            assert!(
                matches!(app.modal, sysview::app::Modal::Explain { .. }),
                "{} 頁按 e 沒有開啟說明",
                view.title()
            );
            app.on_key(Key::Esc);
            app.on_key(Key::Down);
            term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
        }
    }
}

// ── 安全 ────────────────────────────────────────────────────────────────

#[test]
fn describe_never_reads_process_environ() {
    // 原始碼層級的檢查。
    // 說明文字裡提到 environ 是**刻意**的（要告訴使用者我們不讀），
    // 所以只檢查有沒有真的去「讀」它 —— 也就是 environ 出現在檔案讀取的呼叫裡。
    let readers = [
        "read_string",
        "read_to_string",
        "read_into",
        "fs::read",
        "File::open",
    ];
    for f in [
        "src/metrics/describe.rs",
        "src/metrics/describe/entity.rs",
        "src/metrics/describe/current.rs",
    ] {
        let src = std::fs::read_to_string(f).expect(f);
        for line in src.lines() {
            if !line.contains("environ") {
                continue;
            }
            for r in readers {
                assert!(!line.contains(r), "{f} 真的去讀了 environ：{line}");
            }
        }
        // 也不可以組出 environ 的路徑
        assert!(!src.contains("/environ\""), "{f} 組出了 environ 的路徑");
    }
    // 而且輸出裡必須明講我們不讀它
    let s = state();
    let p = s.process.state().processes.first().cloned();
    if let Some(p) = p {
        let c = describe(
            &DescribeTarget::Entity(EntityRef::Process {
                pid: p.pid,
                starttime: p.starttime,
            }),
            &s,
        );
        assert!(
            c.pitfalls.iter().any(|x| x.contains("environ")),
            "行程說明應該明講 sysview 刻意不讀 environ"
        );
        // 但絕不能真的把環境變數的內容放進去
        let dump = format!("{:?}{:?}", c.current, c.summary);
        assert!(!dump.contains("PATH="), "說明裡出現了環境變數內容");
        assert!(!dump.contains("TOKEN"), "說明裡出現了環境變數內容");
    }
}

#[test]
fn describe_never_spawns_a_shell_or_arbitrary_command() {
    for f in [
        "src/metrics/describe.rs",
        "src/metrics/describe/entity.rs",
        "src/metrics/describe/current.rs",
    ] {
        let src = std::fs::read_to_string(f).expect(f);
        for pat in ["Command::new", "sh -c", "process::Command", "popen"] {
            assert!(
                !src.contains(pat),
                "{f} 出現了 {pat:?} —— Describe 只能用已取樣的狀態，不可執行指令"
            );
        }
    }
}

#[test]
fn describe_of_another_users_process_says_permission_is_needed() {
    let s = state();
    let me = sysview::collectors::util::real_uid();
    let other = s.process.state().processes.iter().find(|p| p.uid != me);
    let Some(p) = other else {
        eprintln!("這台機器上看不到別人的行程，略過");
        return;
    };
    let c = describe(
        &DescribeTarget::Entity(EntityRef::Process {
            pid: p.pid,
            starttime: p.starttime,
        }),
        &s,
    );
    assert!(
        c.permission_note.is_some(),
        "別人的行程應該標示更深入的資訊需要管理員權限"
    );
    // 而且不可以真的洩漏出本來看不到的東西
    let text = format!("{:?}", c.current);
    assert!(!text.contains("environ"));
}

#[test]
fn admin_only_metrics_say_so_instead_of_showing_data() {
    let s = state();
    let c = describe(&DescribeTarget::Metric("storage.user"), &s);
    let has_note = c
        .current
        .iter()
        .any(|f| f.value.contains("管理員") || f.value.contains("Admin"));
    assert!(
        has_note,
        "需要管理員權限的 metric 應該講清楚，而不是顯示空資料"
    );
}

#[test]
fn path_describe_handles_unreadable_paths_gracefully() {
    let s = state();
    for p in ["/root", "/proc/1/fd", "/nonexistent-xyz"] {
        let c = describe(&DescribeTarget::Entity(EntityRef::Path(p.into())), &s);
        assert!(!c.current.is_empty(), "{p} 應該至少回報路徑本身");
        assert!(!c.summary.is_empty());
    }
}

#[test]
fn a_machine_without_a_gpu_has_nothing_to_select_on_the_gpu_page() {
    // CI runner 沒有 GPU：GPU 頁只有一句「沒有偵測到」，沒有可選區域。
    // 上面兩個測試對這種機器跳過 GPU 頁，這裡把「跳過的條件」本身釘住。
    let (app, _t) = app_rendered_without_gpu(View::Gpu);
    assert!(gpu_page_is_empty_here(&app, View::Gpu));
    assert!(app.regions.is_empty(), "沒有 GPU 卻登記了可選區域");
    // 其他頁不受影響
    let (app, _t) = app_rendered_without_gpu(View::Cpu);
    assert!(!gpu_page_is_empty_here(&app, View::Cpu));
    assert!(!app.regions.is_empty());
}
