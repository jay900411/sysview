//! Pattern / texture system。
//!
//! # 為什麼需要 pattern
//!
//! 純色長條只能編碼一個維度：長度。但「記憶體由 used / buffers / cache /
//! free 組成」這種資訊，色彩一旦失效（單色終端、色盲、SSH 進到只有 8 色的
//! 機器）就整段讀不出來。Pattern 把「這是哪一段」編碼進**紋理**，
//! 那是一條跟顏色互相獨立的通道。
//!
//! 所以這裡的 pattern **不是裝飾**，是 visual encoding。裝飾可以在小畫面
//! 關掉，pattern 不行 —— 它關掉就等於資訊消失。
//!
//! # 設計約束
//!
//! * 每個 pattern 都必須在單色終端下仍然可分辨。
//! * 字元一律取自終端機字型幾乎都有的區段（Block Elements、Box Drawing、
//!   Braille），不用 emoji、不用私有區。
//! * 全部是 `&'static str`，畫的時候零配置。

use std::fmt;

/// 一種填滿紋理。
///
/// 每個 pattern 提供一組**由疏到密**的字元。畫組成長條時，不同的段落挑
/// 不同的 pattern；畫強度時，同一個 pattern 挑不同的密度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pattern {
    /// 實心。最強、最重，留給「已經用掉、拿不回來」的那一段。
    #[default]
    Solid,
    /// 灰階網點（░▒▓）。傳統、耐看，適合中間層。
    Shade,
    /// 點。很輕，適合「可回收」「快取」這種語意上比較軟的段落。
    Dot,
    /// Braille 點陣。密度細，適合小面積裡還要分層的地方。
    Braille,
    /// 直線。方向性明顯，適合表示「流動中」。
    Line,
    /// 斜線 / 交叉線。像工程圖的剖面線，適合「保留區」「不可用」。
    Hatch,
    /// 數位刻度。像儀器面板，適合表示離散的段。
    Digital,
}

impl Pattern {
    pub const ALL: &'static [Pattern] = &[
        Pattern::Solid,
        Pattern::Shade,
        Pattern::Dot,
        Pattern::Braille,
        Pattern::Line,
        Pattern::Hatch,
        Pattern::Digital,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "solid" => Self::Solid,
            "shade" => Self::Shade,
            "dot" => Self::Dot,
            "braille" => Self::Braille,
            "line" => Self::Line,
            "hatch" => Self::Hatch,
            "digital" => Self::Digital,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::Shade => "shade",
            Self::Dot => "dot",
            Self::Braille => "braille",
            Self::Line => "line",
            Self::Hatch => "hatch",
            Self::Digital => "digital",
        }
    }

    /// 由疏到密的字元梯度。至少 2 階，最多 4 階。
    ///
    /// 最後一個是這個 pattern 最重的樣子，`fill()` 預設用它。
    pub fn ramp(self) -> &'static [char] {
        match self {
            Self::Solid => &['▓', '█'],
            Self::Shade => &['░', '▒', '▓'],
            Self::Dot => &['·', '∙', '•'],
            Self::Braille => &['⠂', '⠒', '⠶', '⣿'],
            Self::Line => &['╵', '│', '┃'],
            Self::Hatch => &['╱', '╳', '▨'],
            // 收在 ▇ 而不是 █：█ 是 Solid 的代表字元，兩個 pattern
            // 的最重階撞在一起，組成長條就分不出段落了（測試抓到過）。
            Self::Digital => &['▁', '▄', '▆', '▇'],
        }
    }

    /// 這個 pattern 的代表字元（圖例、單格指示用）。
    pub fn glyph(self) -> char {
        *self.ramp().last().unwrap_or(&'█')
    }

    /// 依強度 0.0–1.0 取一個字元。
    ///
    /// 用途是「同一段語意、不同強度」，例如 per-core heatmap。
    pub fn at(self, intensity: f64) -> char {
        let r = self.ramp();
        if !intensity.is_finite() || intensity <= 0.0 {
            return r[0];
        }
        let i = (intensity.clamp(0.0, 1.0) * r.len() as f64).ceil() as usize;
        r[i.saturating_sub(1).min(r.len() - 1)]
    }

    /// 單色終端下也分得出來嗎？
    ///
    /// 全部都要回 true —— 這是這個模組存在的理由，有測試在守。
    pub fn distinguishable_without_color(self) -> bool {
        self.glyph() != ' '
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// 一組**互相之間看得出差別**的 pattern，給組成類長條依序取用。
///
/// 順序不是隨便排的：由重到輕，對應「已經用掉 → 可以回收 → 空的」。
/// 讀者不必查圖例，光看紋理輕重就知道哪一段比較「硬」。
pub const COMPOSITION_ORDER: &[Pattern] = &[
    Pattern::Solid,
    Pattern::Shade,
    Pattern::Braille,
    Pattern::Hatch,
    Pattern::Line,
    Pattern::Dot,
    Pattern::Digital,
];

/// 組成長條裡的一段。
#[derive(Debug, Clone)]
pub struct Segment {
    pub label: String,
    pub value: f64,
    pub pattern: Pattern,
    pub color: ratatui::style::Color,
}

/// 把幾個段落畫成一條 pattern-filled bar。
///
/// 回傳每一格的 `(字元, 顏色)`。四捨五入用的是**累積**位置而不是各段各自
/// 取整，這樣總和一定等於 `width`，不會出現最後差一格的縫。
pub fn compose(
    segments: &[Segment],
    width: usize,
    empty: char,
    empty_color: ratatui::style::Color,
) -> Vec<(char, ratatui::style::Color)> {
    let mut out = vec![(empty, empty_color); width];
    if width == 0 {
        return out;
    }
    let total: f64 = segments.iter().map(|s| s.value.max(0.0)).sum();
    if total <= 0.0 {
        return out;
    }
    let mut acc = 0.0;
    let mut cursor = 0usize;
    for seg in segments {
        acc += seg.value.max(0.0);
        // 用累積比例決定邊界，各段的捨入誤差才不會累加
        let end = ((acc / total) * width as f64).round() as usize;
        let end = end.min(width);
        for cell in out.iter_mut().take(end).skip(cursor) {
            *cell = (seg.pattern.glyph(), seg.color);
        }
        cursor = cursor.max(end);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn seg(v: f64, p: Pattern) -> Segment {
        Segment {
            label: String::new(),
            value: v,
            pattern: p,
            color: Color::Reset,
        }
    }

    #[test]
    fn every_pattern_survives_a_monochrome_terminal() {
        // 這是整個模組存在的理由：顏色沒了，紋理還要在
        for p in Pattern::ALL {
            assert!(p.distinguishable_without_color(), "{p} 在單色下消失了");
            assert!(p.ramp().len() >= 2, "{p} 的梯度不足兩階");
        }
    }

    #[test]
    fn patterns_do_not_collide_with_each_other() {
        // 兩個 pattern 的代表字元一樣的話，組成長條就分不出段落
        let mut seen = Vec::new();
        for p in Pattern::ALL {
            assert!(!seen.contains(&p.glyph()), "{p} 的字元跟前面的重複了");
            seen.push(p.glyph());
        }
    }

    #[test]
    fn composition_fills_exactly_the_requested_width() {
        for width in [1usize, 2, 7, 13, 40, 79] {
            let out = compose(
                &[
                    seg(3.0, Pattern::Solid),
                    seg(1.0, Pattern::Shade),
                    seg(2.0, Pattern::Dot),
                ],
                width,
                '·',
                Color::Reset,
            );
            assert_eq!(out.len(), width);
            // 段落加起來要鋪滿，不能留縫
            assert!(
                out.iter().all(|(c, _)| *c != '·'),
                "width={width} 有沒被填到的格子：{:?}",
                out.iter().map(|(c, _)| *c).collect::<String>()
            );
        }
    }

    #[test]
    fn zero_total_leaves_the_bar_empty_instead_of_dividing_by_zero() {
        let out = compose(&[seg(0.0, Pattern::Solid)], 10, '·', Color::Reset);
        assert!(out.iter().all(|(c, _)| *c == '·'));
    }

    #[test]
    fn negative_values_are_treated_as_zero_not_as_a_reversed_segment() {
        let out = compose(
            &[seg(-5.0, Pattern::Solid), seg(1.0, Pattern::Dot)],
            8,
            '·',
            Color::Reset,
        );
        assert!(out.iter().all(|(c, _)| *c == Pattern::Dot.glyph()));
    }

    #[test]
    fn intensity_maps_across_the_whole_ramp() {
        for p in Pattern::ALL {
            let lo = p.at(0.0);
            let hi = p.at(1.0);
            assert_eq!(lo, p.ramp()[0]);
            assert_eq!(hi, p.glyph());
            // NaN 不該 panic，也不該回到最重的那一階
            assert_eq!(p.at(f64::NAN), p.ramp()[0]);
        }
    }

    #[test]
    fn parse_round_trips_every_name() {
        for p in Pattern::ALL {
            assert_eq!(Pattern::parse(p.name()), Some(*p));
            assert_eq!(Pattern::parse(&p.name().to_uppercase()), Some(*p));
        }
        assert_eq!(Pattern::parse("nonsense"), None);
    }
}
