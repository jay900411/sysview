//! 換頁 latency 的逐段拆解。
//!
//! 量的是「按鍵到 stdout flush 完成」這一整段裡，程式自己能控制的每一步：
//! on_key → 版面/狀態 → render 進 Buffer → Buffer::diff → 產生 ANSI →
//! write → flush。事件從終端機到程式的那一段要 PTY 才量得到，
//! 見 `docs/latency.md`。
//!
//! `cargo run --release --example latency`

use std::io::Write;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use sysview::app::{App, Key, View};
use sysview::config::{Branding, Config};
use sysview::theme::ColorDepth;

/// 記下寫進來的位元組數與各自花的時間。
///
/// 真的寫進 `/dev/null`，所以 write / flush 的 syscall 成本是真的 ——
/// 只有「終端機模擬器收到之後做了什麼」量不到，那本來就不在程式手上。
#[derive(Default, Clone, Copy)]
struct Stats {
    bytes: usize,
    write: Duration,
    flush: Duration,
}

#[derive(Clone)]
struct Sink {
    stats: std::rc::Rc<std::cell::RefCell<Stats>>,
    out: std::rc::Rc<std::cell::RefCell<std::fs::File>>,
}

impl Sink {
    fn new() -> Self {
        Self {
            stats: Default::default(),
            out: std::rc::Rc::new(std::cell::RefCell::new(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/null")
                    .expect("/dev/null"),
            )),
        }
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let t = Instant::now();
        let n = self.out.borrow_mut().write(buf)?;
        let mut s = self.stats.borrow_mut();
        s.bytes += n;
        s.write += t.elapsed();
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let t = Instant::now();
        self.out.borrow_mut().flush()?;
        self.stats.borrow_mut().flush += t.elapsed();
        Ok(())
    }
}

fn make(w: u16, h: u16) -> (App, Terminal<CrosstermBackend<Sink>>, Sink) {
    let mut app = App::new(
        Config {
            branding: Branding {
                splash: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ColorDepth::TrueColor,
    );
    app.modal = sysview::app::Modal::None;
    for _ in 0..4 {
        app.tick(Instant::now());
        std::thread::sleep(Duration::from_millis(80));
    }
    let sink = Sink::new();
    let backend = CrosstermBackend::new(sink.clone());
    let mut term = Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, w, h)),
        },
    )
    .expect("term");
    let _ = term.draw(|f| sysview::ui::draw(&app, f));
    (app, term, sink)
}

/// 一次量測：on_key、render、write、flush、合計（µs）與位元組數。
type Row = (u128, u128, u128, u128, u128, usize);

fn key_of(v: View) -> Key {
    match v {
        View::Overview => Key::Char('0'),
        View::Cpu => Key::Char('1'),
        View::Memory => Key::Char('2'),
        View::Gpu => Key::Char('3'),
        View::Storage => Key::Char('4'),
        View::Network => Key::Char('5'),
        View::Process => Key::Char('6'),
        View::Admin => Key::Char('A'),
    }
}

fn main() {
    dino_cost();
    nav_cost();
    sample_cost();
    bytes_per_cell();
    for (w, h) in [(190u16, 48u16), (120, 36)] {
        println!("\n═══ {w}×{h} ═══");
        println!(
            "  {:<22} {:>8} {:>8} {:>8} {:>9} {:>9} {:>8}",
            "換頁", "on_key", "render", "diff+ANSI", "write", "flush", "合計"
        );
        for (from, to) in [
            (View::Cpu, View::Memory),
            (View::Cpu, View::Gpu),
            (View::Cpu, View::Storage),
            (View::Cpu, View::Overview),
            (View::Overview, View::Cpu),
        ] {
            // 每一組都用同樣的次數取中位數，避免第一次的快取效應
            let mut rows: Vec<Row> = Vec::new();
            let (mut app, mut term, sink) = make(w, h);
            for i in 0..14 {
                app.view = from;
                let _ = term.draw(|f| sysview::ui::draw(&app, f));
                *sink.stats.borrow_mut() = Stats::default();

                let t0 = Instant::now();
                app.on_key(key_of(to));
                let t1 = Instant::now();
                let _ = term.draw(|f| sysview::ui::draw(&app, f));
                let t2 = Instant::now();

                if i < 4 {
                    continue; // 暖身
                }
                let s = *sink.stats.borrow();
                let total_draw = t2.duration_since(t1).as_micros();
                let write = s.write.as_micros();
                let flush = s.flush.as_micros();
                rows.push((
                    t1.duration_since(t0).as_micros(),
                    total_draw.saturating_sub(write + flush),
                    write,
                    flush,
                    t2.duration_since(t0).as_micros(),
                    s.bytes,
                ));
            }
            let med = |f: fn(&Row) -> u128| {
                let mut v: Vec<u128> = rows.iter().map(f).collect();
                v.sort_unstable();
                v[v.len() / 2]
            };
            let bytes = {
                let mut v: Vec<usize> = rows.iter().map(|r| r.5).collect();
                v.sort_unstable();
                v[v.len() / 2]
            };
            println!(
                "  {:<22} {:>6}µs {:>6}µs {:>7}µs {:>7}µs {:>7}µs {:>6}µs   {:>6.1} KB",
                format!("{from:?} → {to:?}"),
                med(|r| r.0),
                med(|r| r.1),
                0,
                med(|r| r.2),
                med(|r| r.3),
                med(|r| r.4),
                bytes as f64 / 1024.0,
            );
        }
    }
}

/// 取樣一次要多久。
///
/// 主迴圈是「先吃輸入 → `app.tick()` → 畫」。如果 `tick` 剛好決定要取樣，
/// 那次取樣的成本就夾在**按鍵與畫面之間** —— 大部分按鍵很快，
/// 偶爾一次特別慢，那正是「體感偏慢」的形狀。
fn sample_cost() {
    let mut app = App::new(
        Config {
            branding: Branding {
                splash: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ColorDepth::TrueColor,
    );
    app.modal = sysview::app::Modal::None;
    let mut sampled = Vec::new();
    let mut idle = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(6) {
        let t = Instant::now();
        let did = app.tick(Instant::now());
        let us = t.elapsed().as_micros();
        if did {
            sampled.push(us);
        } else {
            idle.push(us);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let stat = |v: &mut Vec<u128>| {
        if v.is_empty() {
            return (0, 0, 0);
        }
        v.sort_unstable();
        (v[0], v[v.len() / 2], v[v.len() - 1])
    };
    let (a, b, c) = stat(&mut sampled);
    println!(
        "\n  app.tick() 有取樣  {:>3} 次   最小 {a}µs  中位 {b}µs  最大 {c}µs",
        sampled.len()
    );
    let (a, b, c) = stat(&mut idle);
    println!(
        "  app.tick() 沒取樣  {:>3} 次   最小 {a}µs  中位 {b}µs  最大 {c}µs",
        idle.len()
    );
}

/// 換頁時改變的格數，以及每一格花掉多少位元組。
fn bytes_per_cell() {
    use ratatui::backend::TestBackend;
    let mut app = App::new(
        Config {
            branding: Branding {
                splash: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ColorDepth::TrueColor,
    );
    app.modal = sysview::app::Modal::None;
    for _ in 0..4 {
        app.tick(Instant::now());
        std::thread::sleep(Duration::from_millis(80));
    }
    let mut t = Terminal::new(TestBackend::new(190, 48)).expect("term");
    println!(
        "\n  {:<24} {:>8} {:>10} {:>12}",
        "換頁", "改變格數", "位元組", "每格位元組"
    );
    for (from, to) in [
        (View::Cpu, View::Memory),
        (View::Cpu, View::Overview),
        (View::Overview, View::Cpu),
    ] {
        app.view = from;
        t.draw(|f| sysview::ui::draw(&app, f)).expect("draw");
        let a = t.backend().buffer().clone();
        app.view = to;
        t.draw(|f| sysview::ui::draw(&app, f)).expect("draw");
        let b = t.backend().buffer().clone();
        let cells = a.diff(&b).len();

        // 同樣的換頁，量真的送出去的位元組
        let sink = Sink::new();
        let mut real = Terminal::with_options(
            CrosstermBackend::new(sink.clone()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 190, 48)),
            },
        )
        .expect("term");
        app.view = from;
        let _ = real.draw(|f| sysview::ui::draw(&app, f));
        *sink.stats.borrow_mut() = Stats::default();
        app.view = to;
        let _ = real.draw(|f| sysview::ui::draw(&app, f));
        let bytes = sink.stats.borrow().bytes;
        println!(
            "  {:<24} {cells:>8} {:>9.1} KB {:>9.1} B",
            format!("{from:?} → {to:?}"),
            bytes as f64 / 1024.0,
            bytes as f64 / cells.max(1) as f64
        );
    }
}

/// 一次方向鍵讓終端機收到多少東西。
///
/// 換頁重畫整頁是必然的，但「把焦點往下移一格」在概念上只該改兩個框。
/// 如果它也送出十幾 KB，那終端機每按一次方向鍵就要重畫整個畫面 ——
/// 程式端 0.4 毫秒就結束，但眼睛看到的是整頁在刷。
fn nav_cost() {
    use ratatui::backend::TestBackend;
    let mut app = App::new(
        Config {
            branding: Branding {
                splash: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ColorDepth::TrueColor,
    );
    app.modal = sysview::app::Modal::None;
    for _ in 0..3 {
        app.tick(Instant::now());
        std::thread::sleep(Duration::from_millis(60));
    }
    println!(
        "\n  {:<34} {:>9} {:>10}",
        "動作（190×48）", "改變格數", "位元組"
    );
    for (view, name, keys) in [
        (View::Cpu, "CPU：進面板層 ↓", vec![Key::Down]),
        (View::Cpu, "CPU：面板之間 ↓", vec![Key::Down, Key::Down]),
        (View::Cpu, "CPU：進到列 Enter", vec![Key::Down, Key::Enter]),
        (
            View::Cpu,
            "CPU：列與列之間 ↓",
            vec![Key::Down, Key::Enter, Key::Down],
        ),
        (
            View::Process,
            "行程表：列與列之間 ↓",
            vec![Key::Down, Key::Enter, Key::Down],
        ),
        (
            View::Overview,
            "總覽：面板之間 ↓",
            vec![Key::Down, Key::Down],
        ),
    ] {
        app.view = view;
        app.on_key(Key::Esc);
        app.on_key(Key::Esc);
        app.on_key(Key::Esc);
        let mut t = Terminal::new(TestBackend::new(190, 48)).expect("t");
        t.draw(|f| sysview::ui::draw(&app, f)).expect("draw");
        for k in &keys[..keys.len() - 1] {
            app.on_key(*k);
            t.draw(|f| sysview::ui::draw(&app, f)).expect("draw");
        }
        let before = t.backend().buffer().clone();
        app.on_key(*keys.last().unwrap());
        t.draw(|f| sysview::ui::draw(&app, f)).expect("draw");
        let cells = before.diff(t.backend().buffer()).len();

        // 同樣的動作，量真的送出去的位元組
        let sink = Sink::new();
        let mut real = Terminal::with_options(
            CrosstermBackend::new(sink.clone()),
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 190, 48)),
            },
        )
        .expect("t");
        app.view = view;
        app.on_key(Key::Esc);
        app.on_key(Key::Esc);
        app.on_key(Key::Esc);
        let _ = real.draw(|f| sysview::ui::draw(&app, f));
        for k in &keys[..keys.len() - 1] {
            app.on_key(*k);
            let _ = real.draw(|f| sysview::ui::draw(&app, f));
        }
        *sink.stats.borrow_mut() = Stats::default();
        app.on_key(*keys.last().unwrap());
        let _ = real.draw(|f| sysview::ui::draw(&app, f));
        let bytes = sink.stats.borrow().bytes;
        println!("  {name:<34} {cells:>9} {:>7.1} KB", bytes as f64 / 1024.0);
    }
}

/// 彩蛋每一格的成本：改變格數與位元組。
fn dino_cost() {
    use ratatui::backend::TestBackend;
    let mut app = App::new(
        Config {
            branding: Branding {
                splash: false,
                ..Default::default()
            },
            ..Default::default()
        },
        ColorDepth::TrueColor,
    );
    app.modal = sysview::app::Modal::None;
    for _ in 0..3 {
        app.tick(Instant::now());
        std::thread::sleep(Duration::from_millis(60));
    }
    app.on_key(Key::Char('g'));

    let mut t = Terminal::new(TestBackend::new(190, 48)).expect("t");
    let sink = Sink::new();
    let mut real = Terminal::with_options(
        CrosstermBackend::new(sink.clone()),
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 190, 48)),
        },
    )
    .expect("t");
    t.draw(|f| sysview::ui::draw(&app, f)).expect("d");
    let _ = real.draw(|f| sysview::ui::draw(&app, f));

    let mut cells = Vec::new();
    let mut bytes = Vec::new();
    let mut render = Vec::new();
    // 讓它活著才量得到「一格在跑」的成本 —— 撞死之後畫面是靜止的
    let mut now = Instant::now();
    for _ in 0..200 {
        now += sysview::ui::dino::FRAME;
        app.tick_game(now);
        if let Some(g) = app.dino.as_mut() {
            g.autopilot(now);
        }
        let before = t.backend().buffer().clone();
        let t0 = Instant::now();
        t.draw(|f| sysview::ui::draw(&app, f)).expect("d");
        render.push(t0.elapsed().as_micros());
        cells.push(before.diff(t.backend().buffer()).len());
        *sink.stats.borrow_mut() = Stats::default();
        let _ = real.draw(|f| sysview::ui::draw(&app, f));
        bytes.push(sink.stats.borrow().bytes);
    }
    let med = |v: &mut Vec<usize>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    let medu = |v: &mut Vec<u128>| {
        v.sort_unstable();
        v[v.len() / 2]
    };
    println!(
        "\n  恐龍每格：改變 {} 格   {} 位元組   render {}µs   （{:.0} fps → {:.1} KB/s）",
        med(&mut cells),
        med(&mut bytes),
        medu(&mut render),
        1.0 / sysview::ui::dino::FRAME.as_secs_f64(),
        med(&mut bytes) as f64 / sysview::ui::dino::FRAME.as_secs_f64() / 1024.0
    );
}
