//! 換頁成本量測：改變的格數、繪製時間、以及模擬的終端機位元組數。
//!
//! `cargo run --release --example transition_bench`

use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use sysview::app::{App, View};
use sysview::config::{Branding, Config};
use sysview::theme::ColorDepth;

fn make(mascot: &str, w: u16, h: u16) -> (App, Terminal<TestBackend>) {
    let config = Config {
        branding: Branding {
            mascot: mascot.to_owned(),
            splash: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut app = App::new(config, ColorDepth::TrueColor);
    app.tick(Instant::now());
    std::thread::sleep(Duration::from_millis(80));
    app.tick(Instant::now());
    (app, Terminal::new(TestBackend::new(w, h)).expect("term"))
}

fn snapshot(app: &App, term: &mut Terminal<TestBackend>) -> Buffer {
    term.draw(|f| sysview::ui::draw(app, f)).expect("draw");
    term.backend().buffer().clone()
}

/// 粗估一格要寫幾個位元組：移動游標 + 樣式 + 字元。
///
/// 真實數字取決於前後格的樣式差異，這裡取一個保守的常數，
/// 目的是把「改變的格數」換算成可比較的量級，不是精確模擬。
const BYTES_PER_CELL: usize = 12;

fn bench(label: &str, from: View, to: View, mascot: &str, w: u16, h: u16) {
    let (mut app, mut term) = make(mascot, w, h);
    app.view = from;
    let before = snapshot(&app, &mut term);
    app.view = to;

    // 繪製時間（取多次的中位數，排除第一次的配置成本）
    let mut times = Vec::new();
    for _ in 0..40 {
        let t = Instant::now();
        term.draw(|f| sysview::ui::draw(&app, f)).expect("draw");
        times.push(t.elapsed());
    }
    times.sort();
    let draw = times[times.len() / 2];

    let after = term.backend().buffer().clone();
    let changed = before.diff(&after).len();
    println!(
        "  {label:26} 改變 {changed:5} 格   繪製 {:>7.0?}   估計 {:5.1} KB",
        draw,
        (changed * BYTES_PER_CELL) as f64 / 1024.0
    );
}

fn main() {
    for (w, h) in [(190u16, 48u16), (120, 36)] {
        println!("\n═══ {w}×{h} ═══");
        for mascot in ["fox", "none"] {
            println!("  ── mascot = {mascot} ──");
            bench("CPU → Memory", View::Cpu, View::Memory, mascot, w, h);
            bench("Memory → GPU", View::Memory, View::Gpu, mascot, w, h);
            bench("CPU → Overview", View::Cpu, View::Overview, mascot, w, h);
            bench("Overview → CPU", View::Overview, View::Cpu, mascot, w, h);
            bench(
                "Overview → Memory",
                View::Overview,
                View::Memory,
                mascot,
                w,
                h,
            );
            bench("Overview → GPU", View::Overview, View::Gpu, mascot, w, h);
        }
    }
}
