//! 眨眼的座標有沒有真的打在剪影上？沒打中就是白寫一格動畫。
use sysview::ui::visual::mascot::{self, Species, State};

fn main() {
    for sp in [Species::Fox, Species::Deer] {
        for st in State::ALL {
            let p = mascot::pose(sp, *st);
            let (w, h, base) = mascot::rasterize(p, 0);
            let mut hits = 0;
            let mut total = 0;
            for f in p.frames {
                for &(x, y) in f.clear {
                    total += 1;
                    if (x as usize) < w && (y as usize) < h && base[y as usize * w + x as usize] {
                        hits += 1;
                    }
                }
            }
            let has_blink = total > 0;
            println!(
                "{:5} {:8}  眨眼點 {total:2} 個，命中剪影 {hits:2} 個  {}",
                sp.name(),
                st.name(),
                if !has_blink {
                    "✗ 這個姿態沒有眨眼"
                } else if hits == 0 {
                    "✗ 全部落在空白處，眨了等於沒眨"
                } else {
                    "✓"
                }
            );
        }
    }
}
