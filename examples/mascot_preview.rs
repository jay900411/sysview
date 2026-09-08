//! 印出全部吉祥物姿態，給人目視檢查用。
//!
//! `cargo run --release --example mascot_preview > docs/mascots.txt`

use sysview::ui::visual::mascot::{self, Species, State};
use sysview::ui::visual::motif;

fn main() {
    println!("sysview — 吉祥物全集");
    println!(
        "每一隻都是從 concept 圖直接萃取的剪影，56 欄 × 16 列，用四分格繪製（一格 2×2 子像素）。"
    );
    println!(
        "狐狸與鹿各五個姿態。設定用 [branding] mascot = \"fox\" | \"deer\" | \"auto\" | \"none\"。"
    );
    println!("auto 就是狐狸。跟著系統狀態走的是姿態，不是種類。\n");

    for sp in [Species::Fox, Species::Deer] {
        println!("\n{}", "═".repeat(78));
        println!("  {} {}", motif::PREFIX, sp.name());
        println!("{}", "═".repeat(78));
        for st in State::ALL {
            let p = mascot::pose(sp, *st);
            let (cap, line) = st.caption();
            println!("\n╭╴ {} // {}  ── [ {} ]", sp.name(), cap, st.name());
            println!("│  {}", when(*st));
            println!("│");
            let (w, h, bits) = mascot::rasterize(p, 0);
            for l in mascot::to_lines(w, h, &bits) {
                println!("│  {l}");
            }
            println!("│  {}", mascot::ground(56, 0x5157_1E77));
            println!("╰╴ {line}");
        }
    }

    // 動畫：把同一個姿態的每一格並排，看得出差別在哪
    println!("\n\n{}", "═".repeat(78));
    println!("  {} 動畫格（狐狸 · OBSERVE）", motif::PREFIX);
    println!("{}", "═".repeat(78));
    println!("  差異只有：眨眼（關掉四個點）與呼吸（整隻上移一像素）。");
    println!("  存的是「差異」不是整張圖，所以每一格的計算量是幾個座標而已。\n");
    let p = mascot::pose(Species::Fox, State::Observe);
    for f in 0..p.frames.len() {
        let (w, h, bits) = mascot::rasterize(p, f);
        let lines = mascot::to_lines(w, h, &bits);
        println!("  ── 第 {} 格 ──", f + 1);
        // 只印頭部那幾列，眨眼才看得出來
        for l in lines.iter().take(5) {
            println!("  {l}");
        }
        println!();
    }
}

fn when(s: State) -> &'static str {
    match s {
        State::Observe => "一切正常時",
        State::Explore => "有負載但健康時",
        State::Proceed => "高負載時",
        State::Rest => "系統閒置超過 20 秒時",
        State::Return => "剛從吃緊狀態恢復時",
    }
}
