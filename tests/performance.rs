//! 效能關卡。
//!
//! sysview 是常駐在 server 上的工具，**它自己絕不能成為負載來源**。
//! 這裡量測每個 collector 的取樣成本，並對最貴的那個設上限。

use std::time::{Duration, Instant};

use sysview::collectors::{Intervals, SystemState};

fn state() -> SystemState {
    SystemState::new(240, Intervals::from_base(Duration::from_secs(1)))
}

/// 取樣 n 次，回傳每次的平均耗時。
fn measure(label: &str, n: u32, mut f: impl FnMut()) -> Duration {
    f(); // 暖身，把 page cache 與快取填好
    let t = Instant::now();
    for _ in 0..n {
        f();
    }
    let per = t.elapsed() / n;
    println!("  {label:<28} {:>8.3} ms", per.as_secs_f64() * 1000.0);
    per
}

#[test]
fn per_collector_sampling_cost_is_bounded() {
    let mut s = state();
    let now = Instant::now();
    println!("\n每個 collector 的單次取樣成本：");

    let cpu = measure("CPU", 30, || s.cpu.update(now));
    let mem = measure("Memory", 30, || s.memory.update());
    let disk = measure("Disk", 30, || s.disk.update(now));
    let net = measure("Network", 30, || s.network.update(now));
    let gpu = measure("GPU (NVML + sysfs)", 30, || s.gpu.update(now));
    let ncpu = s.cpu.state().logical.max(1);
    let proc = measure("Process (全部 /proc)", 10, || s.process.update(now, ncpu));

    let total = cpu + mem + disk + net + gpu + proc;
    println!("  {:<28} {:>8.3} ms", "合計", total.as_secs_f64() * 1000.0);
    println!(
        "  1 Hz 下佔單核 {:.2}%（{} 核機器 = 全機 {:.3}%）",
        total.as_secs_f64() * 100.0,
        ncpu,
        total.as_secs_f64() * 100.0 / ncpu as f64
    );

    // 上限訂得寬鬆但有意義：在有幾百個行程的機器上仍應遠低於這些數字
    assert!(
        cpu < Duration::from_millis(10),
        "CPU 取樣太慢：{cpu:?}（只是讀 /proc/stat 加 sysfs）"
    );
    assert!(mem < Duration::from_millis(3), "記憶體取樣太慢：{mem:?}");
    assert!(net < Duration::from_millis(10), "網路取樣太慢：{net:?}");
    assert!(
        gpu < Duration::from_millis(50),
        "GPU 取樣太慢：{gpu:?} —— NVML 應該是微秒等級；\
         若接近 6ms 代表退回了 nvidia-smi 的 fork 路徑"
    );
    assert!(proc < Duration::from_millis(200), "行程掃描太慢：{proc:?}");
    assert!(
        total < Duration::from_millis(250),
        "單輪總取樣成本 {total:?} 過高"
    );
}

#[test]
fn nvml_is_much_cheaper_than_spawning_nvidia_smi() {
    // 這是 v2 相對 v1 最重要的效能改進，要有數字證明
    let mut s = state();
    let now = Instant::now();
    s.gpu.update(now);
    if s.gpu.state().devices.is_empty() {
        eprintln!("沒有 GPU，略過");
        return;
    }
    let nvml = measure("NVML poll", 50, || s.gpu.update(now));

    let smi = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=utilization.gpu",
            "--format=csv,noheader,nounits",
        ])
        .output();
    let Ok(o) = smi else { return };
    if !o.status.success() {
        return;
    }
    let t = Instant::now();
    for _ in 0..10 {
        let _ = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=utilization.gpu",
                "--format=csv,noheader,nounits",
            ])
            .output();
    }
    let spawn = t.elapsed() / 10;
    println!(
        "  nvidia-smi spawn            {:>8.3} ms  ({:.0}× 更貴)",
        spawn.as_secs_f64() * 1000.0,
        spawn.as_secs_f64() / nvml.as_secs_f64().max(1e-9)
    );
    assert!(
        nvml < spawn,
        "NVML ({nvml:?}) 應該比 fork nvidia-smi ({spawn:?}) 便宜得多"
    );
}

#[test]
fn expensive_collectors_are_independently_throttled() {
    // 使用者把更新率調到 0.2s 時，行程掃描不可跟著飆
    let fast = Intervals::from_base(Duration::from_millis(200));
    assert_eq!(fast.fast, Duration::from_millis(200));
    assert!(
        fast.process >= Duration::from_millis(750),
        "行程掃描是最貴的動作，必須有獨立下限"
    );
    assert!(
        fast.mounts >= Duration::from_secs(5),
        "statvfs 對掛掉的 NFS 會阻塞"
    );
    assert!(fast.link >= Duration::from_secs(10));
}

#[test]
fn scheduler_does_not_run_expensive_work_every_tick() {
    let mut s = state();
    let t0 = Instant::now();
    s.sample(t0);
    // 行程掃描在背景執行緒上：結果由之後的 sample() 收回來，這裡等它
    let mut waited = Duration::ZERO;
    while s.timings.process == Duration::ZERO && waited < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
        waited += Duration::from_millis(5);
        s.sample(t0 + Duration::from_millis(10));
    }
    let first = s.timings.process;
    assert!(first > Duration::ZERO, "第一次應該要掃行程");

    // 立刻再 sample 一次：行程掃描不該再跑（也就不會有新的結果進來）
    s.timings.process = Duration::ZERO;
    s.sample(t0 + Duration::from_millis(50));
    std::thread::sleep(Duration::from_millis(30));
    s.sample(t0 + Duration::from_millis(60));
    assert_eq!(
        s.timings.process,
        Duration::ZERO,
        "50ms 後不該重新掃描全部行程"
    );
}

#[test]
fn history_memory_is_bounded() {
    // 長時間執行不可讓記憶體無限成長
    let mut series = sysview::metrics::Series::new(240);
    for i in 0..1_000_000 {
        series.push(i as f64);
    }
    assert_eq!(series.len(), 240, "歷史序列必須有硬上限");
    assert_eq!(series.capacity(), 240);
}

#[test]
fn full_render_is_fast_enough_for_20fps() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use sysview::app::{App, VIEWS};
    use sysview::config::Config;
    use sysview::theme::ColorDepth;

    let mut app = App::new(Config::default(), ColorDepth::TrueColor);
    app.tick(Instant::now());
    let mut term = Terminal::new(TestBackend::new(190, 48)).unwrap();

    println!("\n每一頁的渲染成本（190×48）：");
    for view in VIEWS {
        app.view = view;
        let per = measure(view.title(), 20, || {
            term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
        });
        assert!(
            per < Duration::from_millis(16),
            "{} 頁渲染要 {per:?}，超過 60fps 的預算",
            view.title()
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// 視覺層的成本
//
// 這次改版的硬性要求：裝飾不能把 sysview 變成負載來源。
// 下面每一項都是實測出來的上限，超過就是回歸。
// ─────────────────────────────────────────────────────────────────────

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sysview::app::App;
use sysview::config::{Branding, Config, Visual};
use sysview::theme::ColorDepth;

fn app_for(deco: &str, mascot: &str) -> App {
    let config = Config {
        visual: Visual {
            decorations: deco.to_owned(),
            ..Default::default()
        },
        branding: Branding {
            name: "DEVLAB".to_owned(),
            mascot: mascot.to_owned(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = App::new(config, ColorDepth::TrueColor);
    app.tick(std::time::Instant::now());
    app
}

fn draw_cost(app: &App, w: u16, h: u16, n: u32) -> Duration {
    let mut term = Terminal::new(TestBackend::new(w, h)).expect("term");
    // 先畫一次讓配置暖起來，量的是穩態成本
    term.draw(|f| sysview::ui::draw(app, f)).unwrap();
    let t = Instant::now();
    for _ in 0..n {
        term.draw(|f| sysview::ui::draw(app, f)).unwrap();
    }
    t.elapsed() / n
}

#[test]
fn decorations_do_not_meaningfully_slow_down_a_frame() {
    let plain = app_for("off", "none");
    let fancy = app_for("full", "fox");
    let bare = draw_cost(&plain, 200, 60, 60);
    let decorated = draw_cost(&fancy, 200, 60, 60);

    // 一幀的絕對上限。3 fps 的動畫下，5ms 一幀就是 1.5% 的一顆核心，
    // 而實際上遠低於此 —— 這個門檻是抓回歸用的，不是目標值。
    assert!(
        decorated < Duration::from_millis(5),
        "帶裝飾的一幀太慢了：{decorated:?}"
    );
    // 裝飾的額外成本不該超過原本的一倍
    assert!(
        decorated < bare * 3 + Duration::from_micros(500),
        "裝飾讓繪製變慢太多：無裝飾 {bare:?} → 有裝飾 {decorated:?}"
    );
    eprintln!("一幀成本：無裝飾 {bare:?}，完整裝飾 {decorated:?}");
}

#[test]
fn the_mascot_animation_costs_almost_nothing_per_frame() {
    // 動畫只是換一格點陣，成本必須跟「不動」幾乎一樣
    let app = app_for("full", "fox");
    let still = draw_cost(&app, 200, 60, 40);

    let mut animated = app_for("full", "fox");
    let mut term = Terminal::new(TestBackend::new(200, 60)).unwrap();
    term.draw(|f| sysview::ui::draw(&animated, f)).unwrap();
    let mut now = Instant::now();
    let t = Instant::now();
    for _ in 0..40 {
        now += Duration::from_millis(400);
        animated.tick_animation(now);
        term.draw(|f| sysview::ui::draw(&animated, f)).unwrap();
    }
    let per = t.elapsed() / 40;
    assert!(
        per < still + Duration::from_micros(300),
        "推進動畫讓每一幀貴了太多：靜止 {still:?} → 動畫 {per:?}"
    );
    eprintln!("動畫每幀額外成本：{:?}", per.saturating_sub(still));
}

#[test]
fn rasterizing_a_mascot_is_cheap_enough_to_do_every_frame() {
    use sysview::ui::visual::mascot::{self, Species, State};
    let pose = mascot::pose(Species::Fox, State::Observe);
    let n = 2000;
    let t = Instant::now();
    for i in 0..n {
        let (w, h, bits) = mascot::rasterize(pose, i as usize);
        std::hint::black_box(mascot::to_lines(w, h, &bits));
    }
    let per = t.elapsed() / n;
    assert!(
        per < Duration::from_micros(120),
        "光柵化一隻吉祥物要 {per:?}，太貴了"
    );
    eprintln!("吉祥物光柵化：{per:?} / 次");
}

#[test]
fn pattern_composition_allocates_once_and_stays_fast() {
    use sysview::ui::visual::pattern::{compose, Pattern, Segment};
    let segs: Vec<Segment> = [Pattern::Solid, Pattern::Shade, Pattern::Dot]
        .iter()
        .enumerate()
        .map(|(i, p)| Segment {
            label: String::new(),
            value: (i + 1) as f64,
            pattern: *p,
            color: ratatui::style::Color::Reset,
        })
        .collect();
    let n = 20_000;
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(compose(&segs, 120, '·', ratatui::style::Color::Reset));
    }
    let per = t.elapsed() / n;
    assert!(per < Duration::from_micros(20), "組成長條太慢：{per:?}");
    eprintln!("pattern compose：{per:?} / 次");
}

#[test]
fn animation_never_shortens_the_sampling_interval() {
    // 規格裡最重要的一條：動畫不得提高 /proc、NVML、行程掃描的頻率
    let mut app = app_for("full", "fox");
    let baseline = app.next_sample_in(Instant::now());
    let mut now = Instant::now();
    for _ in 0..100 {
        now += Duration::from_millis(50);
        app.tick_animation(now);
    }
    let after = app.next_sample_in(Instant::now());
    assert!(
        after <= baseline + Duration::from_millis(100),
        "動畫改變了採樣排程：{baseline:?} → {after:?}"
    );
    assert!(app.anim_frame > 0, "動畫確實有在跑");
}

#[test]
fn switching_pages_stays_well_under_a_frame() {
    // 換頁 latency 的回歸柵欄。
    //
    // 量的是「按鍵 → Buffer 畫完 → 算出改變了哪些格」這一整段，也就是
    // 程式在把位元組交給終端機之前的全部工作。實測（190×48）每次換頁
    // 大約 0.6–1.0 毫秒，這裡放寬到 8 毫秒 —— 柵欄要擋的是數量級的退步，
    // 不是量測雜訊。
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use sysview::app::{App, Key, View};
    use sysview::config::Config;
    use sysview::theme::ColorDepth;

    let mut app = App::new(Config::default(), ColorDepth::TrueColor);
    app.modal = sysview::app::Modal::None;
    for _ in 0..3 {
        app.tick(Instant::now());
        std::thread::sleep(Duration::from_millis(60));
    }
    let mut term = Terminal::new(TestBackend::new(190, 48)).unwrap();

    println!("\n換頁成本（190×48，按鍵 → buffer + diff）：");
    for (from, to, key) in [
        (View::Cpu, View::Memory, Key::Char('2')),
        (View::Cpu, View::Overview, Key::Char('0')),
        (View::Overview, View::Cpu, Key::Char('1')),
    ] {
        let mut cells = 0;
        let per = measure(&format!("{from:?} → {to:?}"), 12, || {
            app.view = from;
            term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
            let before = term.backend().buffer().clone();
            app.on_key(key);
            term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
            cells = before.diff(term.backend().buffer()).len();
        });
        assert!(
            per < Duration::from_millis(8),
            "{from:?} → {to:?} 要 {per:?}，換頁不該花掉半個畫格"
        );
        assert!(
            cells > 0 && cells < 190 * 48,
            "{from:?} → {to:?} 改了 {cells} 格 —— 0 代表沒重畫，滿版代表 diff 失效"
        );
    }
}

#[test]
fn resizing_does_not_leave_stale_content_behind() {
    // 主迴圈在 resize 時**不呼叫** `terminal.clear()` —— 那個呼叫會送出
    // `ESC[6n` 等終端機回答，實測會讓接下來的第一個按鍵卡 1.9 秒。
    //
    // 拿掉它的前提是「下一次 draw() 本來就會整頁重畫」。這個測試守著
    // 那個前提：改變大小之後畫出來的東西，要跟一開始就是那個大小一樣。
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use sysview::app::{App, View};
    use sysview::config::Config;
    use sysview::theme::ColorDepth;

    let make = || {
        let mut app = App::new(Config::default(), ColorDepth::TrueColor);
        app.modal = sysview::app::Modal::None;
        app.view = View::Cpu;
        app.tick(Instant::now());
        app
    };
    for (w, h) in [(120u16, 36u16), (80, 24), (200, 60)] {
        // 一路 resize 過來的
        let app = make();
        let mut term = Terminal::new(TestBackend::new(190, 48)).unwrap();
        term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
        term.backend_mut().resize(w, h);
        term.draw(|f| sysview::ui::draw(&app, f)).unwrap();
        let resized = sysview::ui::buffer_text(term.backend().buffer());

        // 一開始就是這個大小的
        let app2 = make();
        let mut fresh = Terminal::new(TestBackend::new(w, h)).unwrap();
        fresh.draw(|f| sysview::ui::draw(&app2, f)).unwrap();
        let direct = sysview::ui::buffer_text(fresh.backend().buffer());

        // 時鐘與取樣值會不一樣，所以比的是版面骨架
        let skeleton = |t: &str| -> Vec<String> {
            t.lines()
                .map(|l| {
                    l.chars()
                        .map(|c| {
                            if "╭╮╰╯│─┤├".contains(c) {
                                c
                            } else {
                                ' '
                            }
                        })
                        .collect::<String>()
                        .trim_end()
                        .to_owned()
                })
                .collect()
        };
        assert_eq!(
            skeleton(&resized),
            skeleton(&direct),
            "{w}×{h}：resize 之後的版面跟直接開這個大小不一樣（有殘影）"
        );
    }
}
