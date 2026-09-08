//! 裝飾槽 —— 把 logo、吉祥物、標語畫進一塊指定的區域。
//!
//! 這是整個視覺系統唯一會碰 `Buffer` 的地方，好處是「裝飾能不能畫」
//! 的判斷全部集中在這裡：呼叫端只要把一塊 Rect 交出來，
//! 放不下就什麼都不會發生，不會有半截的圖跑出來撐破版面。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::theme::Theme;
use crate::ui::format;
use crate::ui::visual::logo::Brand;
use crate::ui::visual::mascot::{self, Species, State};
use crate::ui::visual::{motif, Decoration};

/// 完整吉祥物需要的空間（欄 × 列）。
pub const MASCOT_W: u16 = 56;
pub const MASCOT_H: u16 = 16;
/// 這塊區域畫不畫得下吉祥物。
///
/// 只有「畫得下」與「畫不下」兩種，沒有中間的縮小版本 ——
/// 降到一半尺寸時細腿會碎成一格一格，與其塞一隻醜的不如不畫。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    None,
    Full,
}

pub fn fit(area: Rect, deco: Decoration) -> Fit {
    // 留一列給地面
    if deco.at_least(Decoration::Standard) && area.width >= MASCOT_W && area.height > MASCOT_H {
        Fit::Full
    } else {
        Fit::None
    }
}

/// 畫一隻吉祥物要知道的全部事情。
///
/// 包成一個結構是因為參數已經多到讀不出誰是誰了 ——
/// `mascot(buf, area, theme, fox, observe, 3, full, true)` 那種呼叫，
/// 最後兩個布林是什麼意思沒人記得住。
#[derive(Debug, Clone, Copy)]
pub struct Draw<'a> {
    pub species: Species,
    pub state: State,
    /// 動畫格。由 `App::anim_frame` 提供，跟採樣時鐘無關。
    pub frame: u64,
    pub deco: Decoration,
    /// 要不要一起畫狀態標籤與兩行標語。
    pub captioned: bool,
    /// 真的畫出來時設為 true。
    ///
    /// 由這裡回報而不是讓每個呼叫端自己判斷 —— 「畫得下嗎」的邏輯只有
    /// 這個函式知道，交給呼叫端猜遲早會有人猜錯（說明頁就漏過一次）。
    pub drawn: Option<&'a std::cell::Cell<bool>>,
}

/// 把吉祥物畫進 `area`。
///
/// 回傳實際用掉的高度，`0` 代表沒畫。呼叫端可以用它決定下面還能放什麼。
pub fn mascot(buf: &mut Buffer, area: Rect, theme: &Theme, d: Draw) -> u16 {
    let Draw {
        species,
        state,
        frame,
        deco,
        captioned,
        drawn,
    } = d;
    let f = fit(area, deco);
    if f == Fit::None {
        return 0;
    }
    if let Some(flag) = drawn {
        flag.set(true);
    }
    let pose = mascot::pose(species, state);
    let (w, h, bits) = mascot::rasterize(pose, frame as usize);
    let lines = mascot::to_lines(w, h, &bits);

    let ink = theme.style(theme.palette.fg);
    let dim = theme.dim_style();
    let faint = theme.faint_style();

    // 置中
    let art_w = lines.iter().map(|l| format::width(l)).max().unwrap_or(0) as u16;
    let x0 = area.x + area.width.saturating_sub(art_w) / 2;

    let mut y = area.y;
    for line in &lines {
        if y >= area.y + area.height {
            return y - area.y;
        }
        buf.set_string(x0, y, format::truncate(line, area.width as usize), ink);
        y += 1;
    }
    // 地面：讓剪影站在某個地方，而不是浮著
    if y < area.y + area.height {
        let g = mascot::ground(art_w as usize, 0x5157_1E77);
        buf.set_string(x0, y, format::truncate(&g, area.width as usize), faint);
        y += 1;
    }
    if captioned && y + 1 < area.y + area.height {
        let (cap, line) = state.caption();
        let tag = format!("{} {} · {}", motif::PREFIX, species.name(), state.name());
        buf.set_string(
            area.x,
            y,
            format::truncate(&tag, area.width as usize),
            faint,
        );
        y += 1;
        if y < area.y + area.height {
            buf.set_string(area.x, y, format::truncate(cap, area.width as usize), dim);
            y += 1;
        }
        if y < area.y + area.height {
            buf.set_string(
                area.x,
                y,
                format::truncate(line, area.width as usize),
                faint,
            );
            y += 1;
        }
    }
    y - area.y
}

/// 把品牌 logo 畫進一塊**專門留給它**的區域，回傳用掉的高度。
///
/// 呈現方式由「這塊區域實際有多大」決定，不照 dashboard 的裝飾預算走。
/// 兩者問的不是同一件事：[`Decoration`] 問的是「這台終端機還剩多少餘裕
/// 給裝飾」，而啟動畫面與說明頁是**已經把四列留給 logo** 的地方 ——
/// 那四列不會因為終端機小就變少。
///
/// 照預算走的後果實際發生過：160 欄以下一律降到 `Style::Small`，
/// 而 Small 只有兩列，字被壓過一次，看起來就是 logo 被腰斬。
/// 裝飾整個關掉（`Decoration::None`）仍然只畫文字 —— 那是使用者的決定，
/// 或者畫面真的小到不該有裝飾。
pub fn logo(buf: &mut Buffer, area: Rect, theme: &Theme, brand: &Brand, deco: Decoration) -> u16 {
    if area.width == 0 || area.height == 0 {
        return 0;
    }
    let deco = if deco == Decoration::None {
        deco
    } else {
        Decoration::Full
    };
    let lines = brand.render(area.width as usize, area.height as usize, deco);
    let style = theme.bold(theme.palette.accent);
    for (i, line) in lines.iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            return i as u16;
        }
        buf.set_string(
            area.x,
            y,
            format::truncate(line, area.width as usize),
            style,
        );
    }
    lines.len() as u16
}

/// concept 圖那種頁尾標語列：`// FOX  |  …            ‖‖‖‖  EXPLORE BUILD BELONG`
pub fn signature(buf: &mut Buffer, area: Rect, theme: &Theme, brand: &Brand, deco: Decoration) {
    if deco == Decoration::None || area.height == 0 || area.width < 30 {
        return;
    }
    let faint = theme.faint_style();
    let left = crate::ui::visual::logo::tagline(brand);
    buf.set_string(
        area.x,
        area.y,
        format::truncate(&left, area.width as usize),
        faint,
    );
    if deco.at_least(Decoration::Standard) && area.width >= 60 {
        let right = format!("OBSERVE  EXPLAIN  UNDERSTAND {}", motif::SIGNATURE);
        let rw = format::width(&right) as u16;
        if rw + format::width(&left) as u16 + 4 <= area.width {
            buf.set_string(area.x + area.width - rw, area.y, right, faint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorDepth;

    fn t() -> Theme {
        Theme::new("default", ColorDepth::TrueColor)
    }

    #[test]
    fn nothing_is_drawn_when_it_does_not_fit() {
        for (w, h) in [(10u16, 4u16), (30, 6), (55, 10)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
            let used = mascot(
                &mut buf,
                Rect::new(0, 0, w, h),
                &t(),
                Draw {
                    species: Species::Fox,
                    state: State::Observe,
                    frame: 0,
                    deco: Decoration::Full,
                    captioned: true,
                    drawn: None,
                },
            );
            if used == 0 {
                // 沒畫就該是全空白
                let text = crate::ui::buffer_text(&buf);
                assert!(
                    text.trim().is_empty(),
                    "{w}x{h} 說畫不下卻留下了東西：{text:?}"
                );
            }
        }
    }

    #[test]
    fn decorations_off_draws_nothing_at_any_size() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 120, 40));
        let used = mascot(
            &mut buf,
            Rect::new(0, 0, 120, 40),
            &t(),
            Draw {
                species: Species::Fox,
                state: State::Observe,
                frame: 0,
                deco: Decoration::None,
                captioned: true,
                drawn: None,
            },
        );
        assert_eq!(used, 0);
        assert!(crate::ui::buffer_text(&buf).trim().is_empty());
    }

    #[test]
    fn the_mascot_never_draws_outside_its_slot() {
        // 版面爆掉是這次改版最大的風險，所以每個尺寸都要檢查
        for (w, h) in [(60u16, 20u16), (80, 24), (120, 40), (200, 60)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
            let slot = Rect::new(2, 3, w - 4, h - 6);
            for frame in 0..6 {
                for st in State::ALL {
                    mascot(
                        &mut buf,
                        slot,
                        &t(),
                        Draw {
                            species: Species::Deer,
                            state: *st,
                            frame,
                            deco: Decoration::Full,
                            captioned: true,
                            drawn: None,
                        },
                    );
                }
            }
            let text = crate::ui::buffer_text(&buf);
            for (i, line) in text.lines().enumerate() {
                // 要比**顯示寬度**，不是 byte 長度 —— 半格字元一個佔 3 bytes
                let touched = format::width(line.trim_end());
                assert!(touched <= w as usize, "第 {i} 列超出畫面寬度");
                if i < 3 || i >= (h - 3) as usize {
                    assert!(
                        line.trim().is_empty(),
                        "第 {i} 列在槽外卻被畫到了：{line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn fit_degrades_with_space() {
        assert_eq!(fit(Rect::new(0, 0, 60, 20), Decoration::Full), Fit::Full);
        assert_eq!(
            fit(Rect::new(0, 0, 200, 60), Decoration::Standard),
            Fit::Full
        );
        // 放不下就不畫，沒有縮小版本 —— 縮小過的剪影腿會碎成一格一格
        assert_eq!(fit(Rect::new(0, 0, 40, 12), Decoration::Full), Fit::None);
        assert_eq!(fit(Rect::new(0, 0, 20, 6), Decoration::Full), Fit::None);
        assert_eq!(fit(Rect::new(0, 0, 55, 20), Decoration::Full), Fit::None);
        assert_eq!(
            fit(Rect::new(0, 0, 200, 60), Decoration::Minimal),
            Fit::None
        );
    }

    #[test]
    fn logo_reports_the_height_it_actually_used() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 10));
        let b = Brand::new("DEVLAB", "auto");
        let used = logo(
            &mut buf,
            Rect::new(0, 0, 80, 10),
            &t(),
            &b,
            Decoration::Full,
        );
        assert!(used > 0 && used <= 10);
        let text = crate::ui::buffer_text(&buf);
        let drawn = text.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(drawn, used as usize);
    }
}
