//! 逐 frame 檢查：Fox / Deer × Observe / Explore / Proceed / Rest / Return。
//!
//! 產生 `docs/mascot-frames.txt` —— 每個姿態的每一格跟前一格差在哪裡，
//! 加上不變量（腳底、左右範圍、墨量）與循環接回第一格的差異。
//!
//! `cargo run --release --example frame_diff`

use std::fmt::Write as _;

use sysview::ui::visual::mascot::{self, Species, State};

fn bbox(bits: &[bool], w: usize, h: usize) -> (usize, usize, usize, usize) {
    let (mut t, mut b, mut l, mut r) = (h, 0usize, w, 0usize);
    for y in 0..h {
        for x in 0..w {
            if bits[y * w + x] {
                t = t.min(y);
                b = b.max(y);
                l = l.min(x);
                r = r.max(x);
            }
        }
    }
    (t, b, l, r)
}

/// 一組子像素座標。
type Points = Vec<(usize, usize)>;

/// 兩格之間的差異：熄掉的與點亮的子像素座標。
fn diff(a: &[bool], b: &[bool], w: usize) -> (Points, Points) {
    let (mut off, mut on) = (Vec::new(), Vec::new());
    for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
        if x != y {
            let p = (i % w, i / w);
            if *x {
                off.push(p)
            } else {
                on.push(p)
            }
        }
    }
    (off, on)
}

fn coords(v: &[(usize, usize)]) -> String {
    if v.is_empty() {
        return "—".into();
    }
    let s: Vec<String> = v
        .iter()
        .take(8)
        .map(|(x, y)| format!("({x},{y})"))
        .collect();
    if v.len() > 8 {
        format!("{} …共 {}", s.join(" "), v.len())
    } else {
        s.join(" ")
    }
}

fn main() {
    let mut out = String::new();
    out.push_str("sysview 吉祥物 —— 逐 frame 檢查\n");
    out.push_str("=================================\n\n");
    out.push_str(
        "每個姿態列出每一格與**前一格**的差異（熄掉 / 點亮的子像素），\n\
         以及三個不變量：墨量、腳底列、左右範圍。最後一列是循環接回第 0 格。\n\n\
         呼吸 = 把上緣輪廓移動一個子像素（清舊的、點上面那個），所以熄掉與\n\
         點亮的格數相同、墨量不變。這是「不會多描一層邊」的證據。\n\
         眨眼 = 只熄不點。\n\n",
    );

    for sp in [Species::Fox, Species::Deer] {
        for st in State::ALL {
            let pose = mascot::pose(sp, *st);
            let n = pose.frames.len();
            let (w, h, base) = mascot::rasterize(pose, 0);
            let ink0 = base.iter().filter(|b| **b).count();
            let (_, b0, l0, r0) = bbox(&base, w, h);

            let _ = writeln!(
                out,
                "── {} {} ── {n} 格　畫布 {w}×{h} 子像素　背線欄 {:?}　耳朵欄 {:?}",
                sp.name(),
                st.name(),
                pose.back,
                pose.ears
            );
            let _ = writeln!(out, "   基準：墨 {ink0}　腳底 y={b0}　左右 x={l0}..{r0}\n");
            let _ = writeln!(
                out,
                "   格  動作            熄掉                                  點亮                                  墨    腳底  左右"
            );

            let mut prev = base.clone();
            for i in 0..=n {
                let f = &pose.frames[i % n];
                let (_, _, bits) = mascot::rasterize(pose, i % n);
                let (off, on) = diff(&prev, &bits, w);
                let ink = bits.iter().filter(|b| **b).count();
                let (_, b, l, r) = bbox(&bits, w, h);
                let act = match (f.breath, f.ears, f.tail, !f.clear.is_empty()) {
                    (true, ..) => "呼吸",
                    (_, true, ..) => "耳動",
                    (_, _, t, _) if t < 0 => "尾←",
                    (_, _, t, _) if t > 0 => "尾→",
                    (.., true) => "眨眼",
                    _ => "靜止",
                };
                let tag = if i == n {
                    format!("↺{:>2}", 0)
                } else {
                    format!("{i:>3}")
                };
                let _ = writeln!(
                    out,
                    "   {tag} {act:<6} {:<37} {:<37} {ink:>4} {} {}",
                    coords(&off),
                    coords(&on),
                    if b == b0 { " 不動 " } else { " ⚠移動" },
                    if (l, r) == (l0, r0) {
                        "不動"
                    } else {
                        "⚠移動"
                    }
                );
                prev = bits;
            }
            out.push('\n');
        }
    }
    std::fs::write("docs/mascot-frames.txt", &out).expect("寫檔");
    println!("已寫入 docs/mascot-frames.txt（{} 位元組）", out.len());
}
