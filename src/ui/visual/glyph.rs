//! 內建的點陣字型 —— A–Z 與 0–9，共 36 個 glyph。
//!
//! # 為什麼自己刻
//!
//! figlet 之類的外部工具意味著多一個執行期相依、多一次 fork，
//! 而且不同機器裝的字型檔不一樣，同一份設定畫出來會不同 ——
//! 那違反了 sysview「同樣的狀態給同樣的畫面」的原則。
//! 36 個 glyph 用 5×7 點陣手刻進二進位檔，總共不到 2 KB。
//!
//! # 點陣格式
//!
//! 每個 glyph 是 7 列 × 5 欄，`#` 代表點亮。刻意選 5×7 而不是更小：
//! 5×7 是能同時把 `B`/`8`、`O`/`0`、`S`/`5` 分清楚的最小尺寸，
//! 而品牌代號常常同時有字母和數字。
//!
//! 畫出來時用上下半格（`▀▄█`）把兩列壓成一列，所以 7 列的字
//! 只佔 4 個終端機列高。

/// 一個 glyph 的點陣：7 列，每列 5 個字元。
pub type Bitmap = [&'static str; 7];

pub const GLYPH_W: usize = 5;
pub const GLYPH_H: usize = 7;

/// 找出一個字的點陣。只支援 A–Z 0–9 與空白。
///
/// 回傳 `None` 代表這個字沒有點陣 —— 呼叫端要退回文字模式，
/// 不可以畫成空白，那會讓品牌代號缺字而看不出來。
pub fn bitmap(c: char) -> Option<&'static Bitmap> {
    let c = c.to_ascii_uppercase();
    match c {
        ' ' => Some(&SPACE),
        'A'..='Z' => LETTERS.get(c as usize - 'A' as usize),
        '0'..='9' => DIGITS.get(c as usize - '0' as usize),
        _ => None,
    }
}

/// 這個字串能不能整串畫成點陣。
pub fn renderable(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| bitmap(c).is_some())
}

/// 把一串字排成點陣列（每列一個 `String`，`#` 代表點亮）。
///
/// `spacing` 是字距（欄）。回傳固定 7 列。
pub fn layout(s: &str, spacing: usize) -> Option<Vec<String>> {
    if !renderable(s) {
        return None;
    }
    let mut rows = vec![String::new(); GLYPH_H];
    for (i, ch) in s.chars().enumerate() {
        let g = bitmap(ch)?;
        for (r, row) in rows.iter_mut().enumerate() {
            if i > 0 {
                row.push_str(&" ".repeat(spacing));
            }
            row.push_str(g[r]);
        }
    }
    Some(rows)
}

/// 把點陣列壓成半格字元列（兩列點陣 → 一列終端機字元）。
///
/// 這是讓 logo 在終端機裡看起來像真的點陣字的關鍵：一個字元格
/// 可以表達上下兩個點，垂直解析度就翻倍了。
pub fn to_half_blocks(rows: &[String]) -> Vec<String> {
    let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    let mut out = Vec::with_capacity(rows.len().div_ceil(2));
    for pair in rows.chunks(2) {
        let top: Vec<char> = pair[0].chars().collect();
        let bottom: Vec<char> = pair.get(1).map(|r| r.chars().collect()).unwrap_or_default();
        let mut line = String::with_capacity(width);
        for x in 0..width {
            let t = top.get(x).is_some_and(|c| *c == '#');
            let b = bottom.get(x).is_some_and(|c| *c == '#');
            line.push(match (t, b) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        out.push(line);
    }
    out
}

const SPACE: Bitmap = ["     "; 7];

#[rustfmt::skip]
const LETTERS: [Bitmap; 26] = [
    // A
    [" ### ", "#   #", "#   #", "#####", "#   #", "#   #", "#   #"],
    // B
    ["#### ", "#   #", "#   #", "#### ", "#   #", "#   #", "#### "],
    // C
    [" ### ", "#   #", "#    ", "#    ", "#    ", "#   #", " ### "],
    // D
    ["#### ", "#   #", "#   #", "#   #", "#   #", "#   #", "#### "],
    // E
    ["#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#####"],
    // F
    ["#####", "#    ", "#    ", "#### ", "#    ", "#    ", "#    "],
    // G
    [" ### ", "#   #", "#    ", "#  ##", "#   #", "#   #", " ### "],
    // H
    ["#   #", "#   #", "#   #", "#####", "#   #", "#   #", "#   #"],
    // I
    [" ### ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "],
    // J
    ["  ###", "   # ", "   # ", "   # ", "   # ", "#  # ", " ##  "],
    // K
    ["#   #", "#  # ", "# #  ", "##   ", "# #  ", "#  # ", "#   #"],
    // L
    ["#    ", "#    ", "#    ", "#    ", "#    ", "#    ", "#####"],
    // M
    ["#   #", "## ##", "# # #", "#   #", "#   #", "#   #", "#   #"],
    // N
    ["#   #", "##  #", "# # #", "#  ##", "#   #", "#   #", "#   #"],
    // O
    [" ### ", "#   #", "#   #", "#   #", "#   #", "#   #", " ### "],
    // P
    ["#### ", "#   #", "#   #", "#### ", "#    ", "#    ", "#    "],
    // Q
    [" ### ", "#   #", "#   #", "#   #", "# # #", "#  # ", " ## #"],
    // R
    ["#### ", "#   #", "#   #", "#### ", "# #  ", "#  # ", "#   #"],
    // S
    [" ####", "#    ", "#    ", " ### ", "    #", "    #", "#### "],
    // T
    ["#####", "  #  ", "  #  ", "  #  ", "  #  ", "  #  ", "  #  "],
    // U
    ["#   #", "#   #", "#   #", "#   #", "#   #", "#   #", " ### "],
    // V
    ["#   #", "#   #", "#   #", "#   #", "#   #", " # # ", "  #  "],
    // W
    ["#   #", "#   #", "#   #", "#   #", "# # #", "## ##", "#   #"],
    // X
    ["#   #", "#   #", " # # ", "  #  ", " # # ", "#   #", "#   #"],
    // Y
    ["#   #", "#   #", " # # ", "  #  ", "  #  ", "  #  ", "  #  "],
    // Z
    ["#####", "    #", "   # ", "  #  ", " #   ", "#    ", "#####"],
];

#[rustfmt::skip]
const DIGITS: [Bitmap; 10] = [
    // 0 —— 中間加一斜劃，才不會跟 O 混淆
    [" ### ", "#   #", "#  ##", "# # #", "##  #", "#   #", " ### "],
    // 1
    ["  #  ", " ##  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### "],
    // 2
    [" ### ", "#   #", "    #", "   # ", "  #  ", " #   ", "#####"],
    // 3
    ["#####", "   # ", "  #  ", "   # ", "    #", "#   #", " ### "],
    // 4
    ["   # ", "  ## ", " # # ", "#  # ", "#####", "   # ", "   # "],
    // 5
    ["#####", "#    ", "#### ", "    #", "    #", "#   #", " ### "],
    // 6
    ["  ## ", " #   ", "#    ", "#### ", "#   #", "#   #", " ### "],
    // 7
    ["#####", "    #", "   # ", "  #  ", " #   ", " #   ", " #   "],
    // 8
    [" ### ", "#   #", "#   #", " ### ", "#   #", "#   #", " ### "],
    // 9
    [" ### ", "#   #", "#   #", " ####", "    #", "   # ", " ##  "],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_exactly_five_by_seven() {
        for c in ('A'..='Z').chain('0'..='9') {
            let g = bitmap(c).unwrap_or_else(|| panic!("{c} 沒有點陣"));
            assert_eq!(g.len(), GLYPH_H, "{c} 的列數不對");
            for (i, row) in g.iter().enumerate() {
                assert_eq!(row.chars().count(), GLYPH_W, "{c} 第 {i} 列的欄數不對");
                assert!(
                    row.chars().all(|ch| ch == '#' || ch == ' '),
                    "{c} 第 {i} 列有非法字元：{row:?}"
                );
            }
        }
    }

    #[test]
    fn all_thirty_six_glyphs_exist() {
        let n = ('A'..='Z')
            .chain('0'..='9')
            .filter(|c| bitmap(*c).is_some())
            .count();
        assert_eq!(n, 36, "規格要求至少 36 個 glyph");
    }

    #[test]
    fn every_glyph_has_ink() {
        // 空白的 glyph 會讓品牌代號缺字，看起來像壞掉
        for c in ('A'..='Z').chain('0'..='9') {
            let g = bitmap(c).unwrap();
            assert!(g.iter().any(|r| r.contains('#')), "{c} 整個是空的");
        }
    }

    #[test]
    fn easily_confused_pairs_are_actually_different() {
        // 品牌代號會混用字母與數字，這幾組不能長一樣
        for (a, b) in [('O', '0'), ('I', '1'), ('S', '5'), ('B', '8'), ('Z', '2')] {
            assert_ne!(
                bitmap(a).unwrap(),
                bitmap(b).unwrap(),
                "{a} 跟 {b} 的點陣一模一樣，讀不出來"
            );
        }
    }

    #[test]
    fn unsupported_characters_are_reported_not_silently_blanked() {
        for c in ['-', '_', '中', '\n', '\0'] {
            assert!(bitmap(c).is_none(), "{c:?} 不該有點陣");
        }
        assert!(!renderable("CTW-LAB"));
        assert!(!renderable(""));
        assert!(renderable("DEVLAB"));
        assert!(renderable("SYSVIEW01"));
    }

    #[test]
    fn layout_produces_seven_rows_of_equal_width() {
        let rows = layout("AB", 1).expect("可畫");
        assert_eq!(rows.len(), GLYPH_H);
        let w = GLYPH_W * 2 + 1;
        for r in &rows {
            assert_eq!(r.chars().count(), w, "{r:?}");
        }
    }

    #[test]
    fn half_blocks_halve_the_height_and_keep_the_width() {
        let rows = layout("DEVLAB", 1).unwrap();
        let hb = to_half_blocks(&rows);
        assert_eq!(hb.len(), GLYPH_H.div_ceil(2));
        let w = rows[0].chars().count();
        for line in &hb {
            assert_eq!(line.chars().count(), w);
            assert!(
                line.chars().all(|c| matches!(c, '█' | '▀' | '▄' | ' ')),
                "半格輸出只能有這四種字元：{line:?}"
            );
        }
    }
}
