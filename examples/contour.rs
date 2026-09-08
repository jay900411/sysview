//! 印出各姿態的上緣輪廓，用來決定呼吸該作用在哪一段。
use sysview::ui::visual::mascot::{self, Species, State};

fn main() {
    for sp in [Species::Fox, Species::Deer] {
        for st in State::ALL {
            let p = mascot::pose(sp, *st);
            let h = p.art.len();
            let w = p.art[0].chars().count();
            let rows: Vec<Vec<bool>> = p
                .art
                .iter()
                .map(|r| r.chars().map(|c| c == '#').collect())
                .collect();
            // 每一欄的最上緣 y
            let top: Vec<Option<usize>> = (0..w).map(|x| (0..h).find(|&y| rows[y][x])).collect();
            println!("\n{} {}  ({w}×{h})", sp.name(), st.name());
            print!("  top: ");
            for (x, t) in top.iter().enumerate() {
                if x % 4 == 0 {
                    match t {
                        Some(v) => print!("{:>3}", v),
                        None => print!("  ."),
                    }
                }
            }
            println!();
            print!("  x  : ");
            for x in (0..w).step_by(4) {
                print!("{:>3}", x);
            }
            println!();
        }
    }
}
