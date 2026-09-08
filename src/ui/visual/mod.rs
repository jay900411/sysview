//! Visual identity —— pattern、字型、logo、吉祥物、裝飾。
//!
//! 這一層的規矩只有一條：**它永遠不能贏過資料**。
//!
//! 所有東西都掛在 [`Decoration`] 這個層級上，畫面一小就自己退場。
//! 唯一的例外是 [`pattern`] —— 那是 encoding 不是裝飾，關掉等於資訊消失。

pub mod glyph;
pub mod logo;
pub mod mascot;
pub mod motif;
pub mod pattern;
pub mod slot;
pub mod watermark;

use ratatui::layout::Rect;

/// 目前這個畫面能負擔多少裝飾。
///
/// 順序有意義：`None < Minimal < Standard < Full`，比較大小就能判斷
/// 「這個裝飾畫不畫」，不必每個地方各自寫一套尺寸判斷。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Decoration {
    /// 什麼裝飾都不畫。資訊優先。
    None,
    /// 只留最省空間的識別：文字 logo、chip。
    Minimal,
    /// 一般：小 logo、小吉祥物、section motif。
    Standard,
    /// 完整：點陣 logo、完整吉祥物卡片。
    Full,
}

impl Decoration {
    /// 依終端機大小與使用者設定決定裝飾層級。
    ///
    /// 門檻是照著「拿掉裝飾之後，資料還剩多少空間」訂的：
    /// 主要面板需要大約 100 欄才擺得下完整的表格，
    /// 吉祥物要 36 欄，兩者要並存就得 140 欄以上。
    pub fn resolve(setting: &str, area: Rect) -> Self {
        let by_size = if area.width < 90 || area.height < 22 {
            // 這個尺寸下連監控資料都要開始擠了
            Self::None
        } else if area.width < 120 || area.height < 30 {
            Self::Minimal
        } else if area.width < 160 || area.height < 40 {
            Self::Standard
        } else {
            Self::Full
        };
        match setting {
            "off" => Self::None,
            // full 也不是無條件全開：畫面真的太小時仍然要讓位給資料，
            // 不然就會出現「使用者設了 full 結果版面炸掉」。
            "full" => match by_size {
                Self::None => Self::None,
                _ => Self::Full,
            },
            _ => by_size,
        }
    }

    pub fn at_least(self, level: Decoration) -> bool {
        self >= level
    }
}

/// 版面密度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Comfortable,
    Compact,
}

impl Density {
    pub fn resolve(setting: &str, area: Rect) -> Self {
        match setting {
            "comfortable" => Self::Comfortable,
            "compact" => Self::Compact,
            _ if area.height >= 34 && area.width >= 110 => Self::Comfortable,
            _ => Self::Compact,
        }
    }
    /// 面板之間要不要留空行。
    pub fn gap(self) -> u16 {
        match self {
            Self::Comfortable => 1,
            Self::Compact => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn decorations_step_down_as_the_terminal_shrinks() {
        assert_eq!(Decoration::resolve("auto", r(200, 60)), Decoration::Full);
        assert_eq!(
            Decoration::resolve("auto", r(140, 36)),
            Decoration::Standard
        );
        assert_eq!(Decoration::resolve("auto", r(100, 26)), Decoration::Minimal);
        assert_eq!(Decoration::resolve("auto", r(80, 24)), Decoration::None);
        assert_eq!(Decoration::resolve("auto", r(200, 18)), Decoration::None);
    }

    #[test]
    fn off_always_wins() {
        for (w, h) in [(200u16, 60u16), (80, 24), (40, 10)] {
            assert_eq!(Decoration::resolve("off", r(w, h)), Decoration::None);
        }
    }

    #[test]
    fn full_still_backs_off_when_there_is_no_room() {
        // 使用者設 full 也不能讓版面炸掉
        assert_eq!(Decoration::resolve("full", r(200, 60)), Decoration::Full);
        assert_eq!(Decoration::resolve("full", r(120, 32)), Decoration::Full);
        assert_eq!(
            Decoration::resolve("full", r(70, 20)),
            Decoration::None,
            "太小的畫面即使設 full 也要讓位給資料"
        );
    }

    #[test]
    fn ordering_lets_call_sites_ask_at_least() {
        assert!(Decoration::Full.at_least(Decoration::Standard));
        assert!(Decoration::Standard.at_least(Decoration::Minimal));
        assert!(!Decoration::Minimal.at_least(Decoration::Standard));
        assert!(!Decoration::None.at_least(Decoration::Minimal));
    }

    #[test]
    fn density_prefers_compact_when_rows_are_scarce() {
        assert_eq!(Density::resolve("auto", r(160, 50)), Density::Comfortable);
        assert_eq!(Density::resolve("auto", r(160, 24)), Density::Compact);
        assert_eq!(Density::resolve("compact", r(200, 60)), Density::Compact);
        assert_eq!(
            Density::resolve("comfortable", r(60, 16)),
            Density::Comfortable
        );
    }
}
