//! Persistent dashboard mascot layer —— 常駐浮水印。
//!
//! # 為什麼不再是 Overview 上的一格 widget
//!
//! 舊版把吉祥物塞在 Overview 右下角的 `Highlights` 卡片裡。那有三個問題：
//! 只有一頁看得到、要跟資料搶一塊 56×16 的版面、而且那塊版面在
//! Processes 最需要空間的地方。
//!
//! 這一層改成**畫在整個 dashboard 底下的浮水印**：每一頁都在、位置固定、
//! 而且**一格資料都不佔** —— 它只填進畫面上本來就空的地方。
//!
//! # Masked background
//!
//! 浮水印在 body 畫完之後才疊上去，逐格檢查目標是不是空的：
//! 有字、有框線、有長條圖的格子一律跳過。所以它不可能蓋掉資料 ——
//! 不是「盡量避開」，是**結構上做不到**：唯一的寫入條件就是那格是空白。
//!
//! 額外要求左右各 [`CLEARANCE`] 格也是空的。少了這條，剪影會緊貼在
//! 表格右緣的字後面 —— `kworker/R-netns ▄▄██████▛▘` 讀起來像那一列
//! 長出了一條長條圖，而不是背景有隻動物。
//!
//! # 為什麼是「整隻乾淨才畫」
//!
//! 終端機沒有 alpha：一格要嘛是字、要嘛是剪影，不可能兩者都是。所以
//! 「浮水印穿過文字透出來」在這裡做不到 —— 被資料咬掉一角的剪影不會
//! 看起來像浮水印，只會看起來像畫面壞了（早期版本在 Processes 面板裡
//! 就是這樣：`kworker/R-netns ▄▄██████▛▘`）。
//!
//! 因此規則是全有全無：整隻放得進乾淨區域才畫，少一格都不畫。這跟
//! `slot::Fit` 沒有「縮小版」是同一個判斷 —— 半隻不如沒有。
//!
//! # 候選位置
//!
//! 只認死右下角的話，Processes 一滿就永遠看不到。所以給一組**固定順序**
//! 的候選位置，取第一個整隻放得下的。順序固定，所以同一頁同一個尺寸
//! 永遠得到同一個答案，不會每幀跳來跳去。
//!
//! # 跟動畫時鐘的關係
//!
//! `render` 回傳實際畫出來的格數。被資料完全蓋住（例如 Processes 滿版）
//! 時格數是 0，這時**不設** `drawn` 旗標，主迴圈就不會為了一隻看不見的
//! 狐狸醒來。這是「可見才動」規則在浮水印上的實作。

use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::theme::Theme;
use crate::ui::visual::mascot::{self, Species, State};
use crate::ui::visual::Decoration;

/// 浮水印離 body 右下角的距離。
const MARGIN_X: u16 = 2;
const MARGIN_Y: u16 = 1;

/// 墨的左右各要淨空幾格。
const CLEARANCE: u16 = 3;

/// 這一幀要畫的浮水印。
#[derive(Debug, Clone, Copy)]
pub struct Watermark<'a> {
    pub species: Species,
    pub state: State,
    /// 動畫格。跟採樣時鐘無關。
    pub frame: u64,
    pub deco: Decoration,
    /// 真的看得見時才設 true。
    pub drawn: Option<&'a Cell<bool>>,
    /// 上一幀畫在哪裡。還乾淨的話就留在原地 —— 見 [`place`]。
    pub spot: Option<&'a Cell<Option<Rect>>>,
}

/// 把浮水印疊到已經畫好的畫面上。回傳實際畫出來的格數，`0` 代表沒畫。
///
/// 只填**本來就空白**的格子，而且是整塊放得下才畫 —— 缺一角的剪影看
/// 起來是畫面壞了，不是背景有隻動物。
/// 至少要露出幾列才看得出是狐狸還是鹿。
///
/// 逐列看過：兩列只有耳朵尖，是幾團分不出來的墨；三列開始像個頭；
/// **四列**有耳朵、頭頂與吻部，一眼認得出物種。所以下限是四 ——
/// 少於這個就只是雜訊，那不如不畫。
const MIN_REVEAL: u16 = 4;

/// 這個畫面允不允許畫浮水印。
///
/// **只問裝飾政策，不問尺寸。** 尺寸由 [`scan`] 針對「這一次要露幾列」
/// 去判斷 —— 露 4 列的頭只有十幾欄寬，跟整隻的 44 欄是兩回事。
///
/// 早期版本在這裡卡了一道「畫面至少要 118 欄」的閘，而那個數字是照
/// **整隻**的寬度訂的。結果是：終端機只要不夠寬，連只要十幾欄的頭都
/// 不給露 —— 明明上下左右都放得下。那跟「有多少空間就露多少」直接矛盾。
///
/// `Decoration::None`（使用者關掉裝飾，或畫面小到連資料都要擠）仍然
/// 什麼都不畫。
pub fn fits(_area: Rect, deco: Decoration) -> bool {
    deco.at_least(Decoration::Minimal)
}

/// 把浮水印疊到已經畫好的畫面上。回傳實際畫出來的格數，`0` 代表沒畫。
///
/// 只填**本來就空白**的格子，而且是整塊放得下才畫 —— 缺一角的剪影看
/// 起來是畫面壞了，不是背景有隻動物。
pub fn render(buf: &mut Buffer, area: Rect, theme: &Theme, w: Watermark) -> usize {
    if !fits(area, w.deco) {
        if let Some(c) = w.spot {
            c.set(None);
        }
        return 0;
    }
    let full = mascot::pose(w.species, w.state).art.len() as u16 / 2;

    // 露出多少列由**當下真正乾淨的垂直深度**決定：從整隻開始往下試，
    // 第一個放得進去的就是答案。空間多就整隻，少就只探出頭。
    let mut sizes: Vec<(u16, u16, u16)> = Vec::new(); // (列數, 寬, 高)
    let mut rows = full;
    while rows >= MIN_REVEAL {
        let a = mascot::reveal(w.species, w.state, 0, rows);
        if a.w > 0 {
            sizes.push((rows, a.w, a.h));
        }
        rows -= 1;
    }
    let last = w.spot.and_then(|c| c.get());
    let Some((rows, at)) = place_reveal(buf, area, &sizes, last) else {
        if let Some(c) = w.spot {
            c.set(None);
        }
        return 0;
    };
    if let Some(c) = w.spot {
        c.set(Some(at));
    }

    let art = mascot::reveal(w.species, w.state, w.frame, rows);
    let style = theme.watermark_style();
    let mut painted = 0usize;
    for (dy, line) in art.lines.iter().enumerate().take(at.height as usize) {
        for (dx, ch) in line.chars().enumerate() {
            if ch == ' ' || dx >= at.width as usize {
                continue;
            }
            if let Some(c) = buf.cell_mut((at.x + dx as u16, at.y + dy as u16)) {
                c.set_symbol(&ch.to_string());
                c.set_style(style);
                painted += 1;
            }
        }
    }
    if painted > 0 {
        if let Some(flag) = w.drawn {
            flag.set(true);
        }
    }
    painted
}

/// 從最完整的一種開始試，回傳第一個放得下的 `(列數, 位置)`。
///
/// 舊位置還乾淨就沿用（連列數一起），這樣切頁時剪影不會忽大忽小、
/// 也不會瞬移 —— 那比露得多一點重要。
fn place_reveal(
    buf: &Buffer,
    area: Rect,
    sizes: &[(u16, u16, u16)],
    last: Option<Rect>,
) -> Option<(u16, Rect)> {
    // 先決定**這一幀最多能露幾列**，再決定放哪裡。
    //
    // 早期版本反過來：舊位置只要還乾淨就直接沿用，連列數一起。結果是
    // 從密集的頁面切到空曠的頁面時，小小的一顆頭留在原地不會長回來 ——
    // 只有舊位置剛好被新頁面的資料蓋到的頁面（CPU、記憶體）才會重新找。
    // 使用者看到的是「大→小會自動縮，小→大卻不會放大」。
    //
    // 位置的黏性留著（同樣大小、舊位置還乾淨就不動），那是為了切頁時
    // 不瞬移；但大小永遠取當下放得下的最大值。
    let blocked = Blocked::of(buf);
    for (rows, w, h) in sizes {
        if let Some(r) = last {
            if r.width == *w && r.height == *h && is_clear(buf, area, r) {
                return Some((*rows, r));
            }
        }
        if let Some(at) = scan(&blocked, area, *w, *h) {
            return Some((*rows, at));
        }
    }
    None
}

/// 這個位置現在還乾淨嗎（含左右淨空）。
fn is_clear(buf: &Buffer, area: Rect, r: Rect) -> bool {
    let a = *buf.area();
    if r.x < area.x || r.y < area.y {
        return false;
    }
    if r.x + r.width > area.x + area.width || r.y + r.height > area.y + area.height {
        return false;
    }
    let x0 = r.x.saturating_sub(CLEARANCE).max(a.x);
    let x1 = (r.x + r.width + CLEARANCE).min(a.x + a.width);
    for y in r.y..r.y + r.height {
        for x in x0..x1 {
            match buf.cell((x, y)) {
                Some(c) => {
                    let t = c.symbol();
                    if !(t.is_empty() || t == " ") {
                        return false;
                    }
                }
                None => return false,
            }
        }
    }
    true
}

/// 「哪些格子已經有東西」的二維前綴和。
///
/// 有了它，任何矩形乾不乾淨都是 O(1)。沒有的話，光是「露幾列」就要對
/// 十幾種尺寸各掃過整個畫面，每幀幾百萬次比較。建一次、所有尺寸共用。
struct Blocked {
    w: usize,
    h: usize,
    sum: Vec<u32>,
}

impl Blocked {
    fn of(buf: &Buffer) -> Self {
        let a = *buf.area();
        let (w, h) = (a.width as usize, a.height as usize);
        let mut sum = vec![0u32; (w + 1) * (h + 1)];
        for y in 0..h {
            for x in 0..w {
                let used = match buf.cell((x as u16, y as u16)) {
                    Some(c) => {
                        let t = c.symbol();
                        !(t.is_empty() || t == " ")
                    }
                    None => true,
                };
                sum[(y + 1) * (w + 1) + x + 1] =
                    sum[y * (w + 1) + x + 1] + sum[(y + 1) * (w + 1) + x] - sum[y * (w + 1) + x]
                        + u32::from(used);
            }
        }
        Self { w, h, sum }
    }

    fn count(&self, r: Rect) -> u32 {
        let (x0, y0) = (r.x as usize, r.y as usize);
        let (x1, y1) = (x0 + r.width as usize, y0 + r.height as usize);
        if x1 > self.w || y1 > self.h {
            return u32::MAX;
        }
        let w = self.w + 1;
        self.sum[y1 * w + x1] + self.sum[y0 * w + x0]
            - self.sum[y0 * w + x1]
            - self.sum[y1 * w + x0]
    }
}

/// 找一個 `mw × mh` 的乾淨位置（含左右淨空）。找不到就是 `None`。
///
/// 由右下往左上找 —— 右下角是資料密度最低的角落。位置對齊格點：逐格掃
/// 的話，數字寬度變一個字（`9%` → `10%`）就可能讓最佳位置左移一格，
/// 畫面上看起來像剪影在抖。
fn scan(blocked: &Blocked, area: Rect, mw: u16, mh: u16) -> Option<Rect> {
    if mw == 0 || mh == 0 {
        return None;
    }
    const STEP_X: u16 = 4;
    const STEP_Y: u16 = 2;
    let x_lo = area.x + MARGIN_X + CLEARANCE;
    let x_hi = (area.x + area.width).checked_sub(mw + MARGIN_X)?;
    let y_lo = area.y + MARGIN_Y;
    let y_hi = (area.y + area.height).checked_sub(mh + MARGIN_Y)?;
    if x_hi < x_lo || y_hi < y_lo {
        return None;
    }
    let mut y = y_hi;
    loop {
        let mut x = x_hi;
        loop {
            // 淨空要夾在畫面內：越界的話 `count` 會回 MAX，最靠邊的位置
            // 就永遠選不上，剪影會莫名其妙離右緣三格遠。
            let px = x.saturating_sub(CLEARANCE);
            let px1 = (x + mw + CLEARANCE).min(blocked.w as u16);
            let probe = Rect {
                x: px,
                y,
                width: px1 - px,
                height: mh,
            };
            if blocked.count(probe) == 0 {
                return Some(Rect {
                    x,
                    y,
                    width: mw,
                    height: mh,
                });
            }
            if x < x_lo + STEP_X {
                break;
            }
            x -= STEP_X;
        }
        if y < y_lo + STEP_Y {
            break;
        }
        y -= STEP_Y;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorDepth;
    use ratatui::style::Style;

    fn t() -> Theme {
        Theme::new("default", ColorDepth::TrueColor)
    }

    fn wm<'a>(
        drawn: Option<&'a Cell<bool>>,
        spot: Option<&'a Cell<Option<Rect>>>,
    ) -> Watermark<'a> {
        Watermark {
            species: Species::Fox,
            state: State::Observe,
            frame: 0,
            deco: Decoration::Full,
            drawn,
            spot,
        }
    }

    /// 每 `every` 列鋪一整列文字，中間留下空白帶 —— 密集儀表板的形狀。
    fn striped(area: Rect, every: u16) -> Buffer {
        let mut buf = Buffer::empty(area);
        for y in 0..area.height {
            if y % every == 0 {
                buf.set_string(0, y, "x".repeat(area.width as usize), Style::default());
            }
        }
        buf
    }

    #[test]
    fn the_watermark_never_overwrites_anything() {
        // 這是整層存在的前提：它只能填空白格。
        let area = Rect::new(0, 0, 190, 44);
        let mut buf = striped(area, 3);
        let before = buf.clone();
        render(&mut buf, area, &t(), wm(None, None));
        for y in 0..area.height {
            for x in 0..area.width {
                let b = before.cell((x, y)).unwrap().symbol().to_owned();
                if b != " " && !b.is_empty() {
                    assert_eq!(
                        buf.cell((x, y)).unwrap().symbol(),
                        b,
                        "({x},{y}) 本來有資料卻被浮水印蓋掉了"
                    );
                }
            }
        }
    }

    #[test]
    fn a_full_page_of_data_leaves_no_room_and_no_animation() {
        let area = Rect::new(0, 0, 190, 44);
        let mut buf = Buffer::empty(area);
        for y in 0..44 {
            buf.set_string(0, y, "#".repeat(190), Style::default());
        }
        let flag = Cell::new(false);
        assert_eq!(render(&mut buf, area, &t(), wm(Some(&flag), None)), 0);
        assert!(!flag.get(), "完全放不下卻還是要求動畫繼續跑");
    }

    #[test]
    fn an_empty_page_shows_the_whole_silhouette() {
        let area = Rect::new(0, 0, 190, 44);
        let mut buf = Buffer::empty(area);
        let flag = Cell::new(false);
        let painted = render(&mut buf, area, &t(), wm(Some(&flag), None));
        let full = mascot::reveal(Species::Fox, State::Observe, 0, u16::MAX);
        let ink: usize = full
            .lines
            .iter()
            .map(|l| l.chars().filter(|c| *c != ' ').count())
            .sum();
        assert_eq!(painted, ink, "空白頁上沒有整隻畫出來");
        assert!(flag.get());
    }

    #[test]
    fn the_reveal_shrinks_as_the_page_gets_denser() {
        // 這是這一層的重點：空間多就露多，空間少就只探出頭。
        let area = Rect::new(0, 0, 190, 44);
        let mut seen: Vec<usize> = Vec::new();
        for every in [40u16, 12, 9, 7, 6] {
            let mut buf = striped(area, every);
            let spot = Cell::new(None);
            let painted = render(&mut buf, area, &t(), wm(None, Some(&spot)));
            let rows = spot.get().map(|r| r.height as usize).unwrap_or(0);
            assert!(painted > 0, "空白帶有 {} 列卻什麼都沒畫", every - 1);
            seen.push(rows);
        }
        assert!(
            seen.windows(2).all(|w| w[0] >= w[1]),
            "空白帶越窄露出的列數應該越少，實際是 {seen:?}"
        );
        assert!(
            seen[0] > seen[seen.len() - 1],
            "從頭到尾都露一樣多：{seen:?}"
        );
        assert!(
            *seen.iter().min().unwrap() >= MIN_REVEAL as usize,
            "露得比辨識下限還少：{seen:?}"
        );
    }

    #[test]
    fn animation_never_changes_how_much_is_revealed() {
        // 呼吸、眨眼、翹尾巴都不能讓露出的列數或位置跳動 ——
        // 那會看起來像整隻在上下抖。
        let area = Rect::new(0, 0, 190, 44);
        for sp in [Species::Fox, Species::Deer] {
            for st in State::ALL {
                let n = mascot::pose(sp, *st).frames.len();
                let mut fixed: Option<Rect> = None;
                for f in 0..n as u64 {
                    let mut buf = striped(area, 9);
                    let spot = Cell::new(fixed);
                    render(
                        &mut buf,
                        area,
                        &t(),
                        Watermark {
                            species: sp,
                            state: *st,
                            frame: f,
                            deco: Decoration::Full,
                            drawn: None,
                            spot: Some(&spot),
                        },
                    );
                    let at = spot.get().expect("該畫得下");
                    match fixed {
                        None => fixed = Some(at),
                        Some(prev) => assert_eq!(
                            at,
                            prev,
                            "{} {} 第 {f} 格的露出範圍變了",
                            sp.name(),
                            st.name()
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn a_dense_page_still_gets_a_recognisable_mascot() {
        // 面板鋪滿整頁、只剩八列空白帶時，完整剪影放不下，
        // 但仍然要看得到 —— 而且要看得出是什麼。
        let area = Rect::new(0, 0, 190, 44);
        let mut buf = striped(area, 9);
        let flag = Cell::new(false);
        let painted = render(&mut buf, area, &t(), wm(Some(&flag), None));
        assert!(painted > 40, "密集頁面上只畫了 {painted} 格，看不出是什麼");
        assert!(flag.get());
        for y in (0..44).step_by(9) {
            for x in 0..190u16 {
                assert_eq!(
                    buf.cell((x, y)).unwrap().symbol(),
                    "x",
                    "第 {y} 列的資料被蓋掉了"
                );
            }
        }
    }

    #[test]
    fn the_reveal_grows_back_when_a_page_has_more_room() {
        // 密集頁面只露一顆頭；切到空曠的頁面，牠要長回整隻 ——
        // 不能因為那顆頭的位置還乾淨就永遠只露一顆頭。
        let area = Rect::new(0, 0, 190, 44);
        let spot = Cell::new(None);
        let mut dense = striped(area, 9);
        render(&mut dense, area, &t(), wm(None, Some(&spot)));
        let small = spot.get().expect("密集頁面也該露一點").height;
        assert!(small < 12, "密集頁面就露了 {small} 列，測試前提不成立");

        let mut roomy = Buffer::empty(area);
        render(&mut roomy, area, &t(), wm(None, Some(&spot)));
        let big = spot.get().expect("空曠頁面該畫得下").height;
        assert!(
            big > small,
            "從密集頁切到空曠頁之後還是只露 {big} 列（之前 {small}）"
        );
        // 而且同樣的頁面再畫一次，位置與大小都不變（黏性還在）
        let fixed = spot.get();
        let mut again = Buffer::empty(area);
        render(&mut again, area, &t(), wm(None, Some(&spot)));
        assert_eq!(spot.get(), fixed, "同一個畫面連畫兩次，位置卻變了");
    }

    #[test]
    fn a_still_clean_spot_is_kept_so_the_silhouette_does_not_jump() {
        let area = Rect::new(0, 0, 190, 44);
        let mut buf = Buffer::empty(area);
        let spot = Cell::new(None);
        render(&mut buf, area, &t(), wm(None, Some(&spot)));
        let first = spot.get().expect("該畫得下");
        for _ in 0..5 {
            let mut buf = Buffer::empty(area);
            render(&mut buf, area, &t(), wm(None, Some(&spot)));
            assert_eq!(spot.get(), Some(first), "同樣的畫面換了位置");
        }
    }

    #[test]
    fn the_gate_is_the_decoration_policy_not_the_full_mascot_width() {
        // 露 4 列的頭只有十幾欄寬。拿「整隻的 44 欄」當閘門的話，
        // 終端機只要不夠寬就連頭都不給露 —— 明明上下左右都放得下。
        for (w, h) in [(100u16, 44u16), (110, 50), (96, 40)] {
            let area = Rect::new(0, 0, w, h);
            assert!(
                fits(area, Decoration::Minimal),
                "{w}×{h} 應該還是畫得下一點點"
            );
            let mut buf = striped(area, 9);
            assert!(
                render(&mut buf, area, &t(), wm(None, None)) > 0,
                "{w}×{h} 有空白帶卻什麼都沒露"
            );
        }
        // 使用者關掉裝飾、或畫面小到連資料都要擠 —— 那才什麼都不畫
        assert!(!fits(Rect::new(0, 0, 200, 60), Decoration::None));
        let area = Rect::new(0, 0, 200, 60);
        let mut buf = Buffer::empty(area);
        let flag = Cell::new(false);
        assert_eq!(
            render(
                &mut buf,
                area,
                &t(),
                Watermark {
                    deco: Decoration::None,
                    ..wm(Some(&flag), None)
                }
            ),
            0
        );
        assert!(!flag.get());
    }

    #[test]
    fn the_watermark_stays_inside_the_area() {
        let full = Rect::new(0, 0, 200, 50);
        let area = Rect::new(4, 3, 190, 44);
        for sp in [Species::Fox, Species::Deer] {
            for st in State::ALL {
                for frame in 0..8u64 {
                    let mut buf = Buffer::empty(full);
                    render(
                        &mut buf,
                        area,
                        &t(),
                        Watermark {
                            species: sp,
                            state: *st,
                            frame,
                            deco: Decoration::Full,
                            drawn: None,
                            spot: None,
                        },
                    );
                    for y in 0..full.height {
                        for x in 0..full.width {
                            let inside = x >= area.x
                                && x < area.x + area.width
                                && y >= area.y
                                && y < area.y + area.height;
                            if !inside {
                                assert_eq!(
                                    buf.cell((x, y)).unwrap().symbol(),
                                    " ",
                                    "{sp:?} {st:?} 第 {frame} 格畫到了區域外的 ({x},{y})"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_frame_of_every_pose_is_safe_to_draw() {
        let area = Rect::new(0, 0, 190, 44);
        for sp in [Species::Fox, Species::Deer] {
            for st in State::ALL {
                for frame in 0..40u64 {
                    let mut buf = striped(area, 9);
                    render(
                        &mut buf,
                        area,
                        &t(),
                        Watermark {
                            species: sp,
                            state: *st,
                            frame,
                            deco: Decoration::Full,
                            drawn: None,
                            spot: None,
                        },
                    );
                }
            }
        }
    }
}
