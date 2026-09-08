//! 逐 frame 檢查呼吸：確認輪廓在「移動」而不是「變厚」，
//! 而且腳底 / 地面 / 尾巴根 / 鹿角都沒動。
use sysview::ui::visual::mascot::{self, Species, State};

fn main() {
    for sp in [Species::Fox, Species::Deer] {
        for st in State::ALL {
            let p = mascot::pose(sp, *st);
            let (w, h, base) = mascot::rasterize(p, 0);
            let ink0 = base.iter().filter(|b| **b).count();
            // 找出基準幀的腳底（最下面一列有 ink 的位置）與整體 bbox
            let bbox = |bits: &[bool]| {
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
            };
            let (_, b0, l0, r0) = bbox(&base);

            let mut worst_ink = 0i64;
            let mut baseline_moved = false;
            let mut side_moved = false;
            for f in 0..p.frames.len() {
                let (_, _, bits) = mascot::rasterize(p, f);
                let ink = bits.iter().filter(|b| **b).count();
                worst_ink = worst_ink.max((ink as i64 - ink0 as i64).abs());
                let (_, bb, ll, rr) = bbox(&bits);
                if bb != b0 {
                    baseline_moved = true;
                }
                if ll != l0 || rr != r0 {
                    side_moved = true;
                }
            }
            println!(
                "{:5} {:8}  {:2} 格  ink 最大變動 {:3}（{:.1}%）  腳底移動 {}  左右移動 {}",
                sp.name(),
                st.name(),
                p.frames.len(),
                worst_ink,
                worst_ink as f64 / ink0 as f64 * 100.0,
                if baseline_moved { "是 ✗" } else { "否 ✓" },
                if side_moved { "是 ✗" } else { "否 ✓" },
            );
        }
    }
}
