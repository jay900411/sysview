//! 裝飾紋樣 —— 虛線規、遙測刻度、電路感線條。
//!
//! # 只能出現在這些地方
//!
//! * splash
//! * help
//! * admin locked
//! * 明確的裝飾槽
//! * 空狀態 / 閒置狀態
//!
//! **不可以出現在監控資料區。** 這條規則有測試在守：資料面板裡不該
//! 出現這裡的任何字元。理由很簡單 —— 讀數字的人不需要背景噪音，
//! 而一旦紋樣跟資料混在一起，就分不出哪個是量測值哪個是裝飾。

/// concept 圖上那種虛線分隔規。
///
/// `▏` 起頭、中間長橫、尾端幾個短點，像儀器面板的刻度尺。
pub fn rule(width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if width < 8 {
        return "─".repeat(width);
    }
    let tail = "╴╴╴";
    let body = width - 1 - tail.chars().count();
    let mut s = String::with_capacity(width);
    s.push('▏');
    s.push_str(&"─".repeat(body));
    s.push_str(tail);
    s
}

/// 同一條規則線，但**不帶 `▏` 起頭** —— 給左邊已經有框線的地方用。
///
/// 起頭那一格是實心方塊，字體會把它畫得比細橫線亮得多；緊貼在視窗框線
/// 旁邊時看起來像一根多出來的白棒（啟動畫面，使用者回報過）。
pub fn rule_plain(width: usize) -> String {
    if width < 8 {
        return "─".repeat(width);
    }
    let tail = "╴╴╴";
    let mut s = "─".repeat(width - tail.chars().count());
    s.push_str(tail);
    s
}

/// 虛線規：適合裝飾卡片的上下緣，比實線輕。
pub fn dashed(width: usize) -> String {
    const CYCLE: [char; 4] = ['╌', '╌', '╌', ' '];
    (0..width).map(|i| CYCLE[i % CYCLE.len()]).collect()
}

/// 遙測刻度列：長短交錯的刻度，像示波器的底標。
pub fn ticks(width: usize) -> String {
    (0..width)
        .map(|i| match i % 10 {
            0 => '┼',
            5 => '┴',
            _ => '─',
        })
        .collect()
}

/// 電路感的一列。很淡，只有轉折點與接點。
///
/// `seed` 決定走線，同一個 seed 永遠畫出同一條 —— 畫面不能每次重畫都在跳。
pub fn circuit(width: usize, seed: u64) -> String {
    const PIECES: [char; 6] = ['─', '─', '┬', '─', '┴', '·'];
    // 不能用 `seed | 1` 攪動 —— 那會把 42 跟 43 變成同一個種子，
    // 兩條本來該不同的走線就長得一模一樣了（測試抓到過）。
    let mut r = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x5DEE_CE66_D000_0001);
    (0..width)
        .map(|_| {
            r = r
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            PIECES[((r >> 33) % PIECES.len() as u64) as usize]
        })
        .collect()
}

/// 角落標記：`┌╴` 這種很輕的框角，給裝飾槽用。
pub const CORNER_TL: &str = "╭╴";
pub const CORNER_TR: &str = "╶╮";
pub const CORNER_BL: &str = "╰╴";
pub const CORNER_BR: &str = "╶╯";

/// concept 圖右上角那組短橫。
pub const SIGNATURE: &str = "╴╴╴╴";

/// 標題前綴，沿用 concept 的 `//`。
pub const PREFIX: &str = "//";

/// 區段起始記號。分頁列右邊那條 `▏─────╴╴╴` 用的就是它，
/// 所以用同一個字元開頭的一列會被讀成「sysview 的一個區段」。
pub const SECTION: &str = "▏";

/// 這個模組會用到的全部字元。測試用它檢查資料區有沒有被污染。
pub const MOTIF_CHARS: &[char] = &['▏', '╴', '╌', '┼', '┴', '┬', '╭', '╮', '╰', '╯', '╶', '·'];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::format::width as dw;

    #[test]
    fn every_motif_renders_exactly_the_requested_width() {
        for w in [0usize, 1, 3, 7, 8, 20, 79, 200] {
            assert_eq!(dw(&rule(w)), w, "rule({w})");
            assert_eq!(dw(&dashed(w)), w, "dashed({w})");
            assert_eq!(dw(&ticks(w)), w, "ticks({w})");
            assert_eq!(dw(&circuit(w, 3)), w, "circuit({w})");
        }
    }

    #[test]
    fn motifs_are_single_width_so_they_cannot_break_a_border() {
        // 全形字混進來會把框線推歪，那是 v1 踩過的坑
        for s in [rule(40), dashed(40), ticks(40), circuit(40, 1)] {
            assert_eq!(s.chars().count(), dw(&s), "{s:?} 含有寬度不是 1 的字元");
        }
    }

    #[test]
    fn circuit_is_deterministic() {
        assert_eq!(circuit(30, 42), circuit(30, 42));
        assert_ne!(circuit(30, 42), circuit(30, 43));
    }

    #[test]
    fn rule_degrades_gracefully_when_it_is_too_narrow_for_the_tail() {
        for w in 1..8 {
            let r = rule(w);
            assert_eq!(dw(&r), w);
            assert!(!r.contains('╴'), "太窄時不該硬塞裝飾尾巴");
        }
    }
}
