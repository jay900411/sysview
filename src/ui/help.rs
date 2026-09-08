//! 說明視窗。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};

use crate::theme::Theme;
use crate::ui::layout;

/// (區塊標題, [(按鍵, 說明)])
const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "導航：像螢幕選單一樣，一層一層進去",
        &[
            ("← →", "在分頁層時切換分頁"),
            ("↓ / Enter", "進入這一頁 / 往下鑽一層"),
            (
                "← ↑ ↓ →",
                "在同一層的方塊之間移動（依畫面上的實際相對位置）",
            ),
            ("Esc", "回到上一層"),
            ("", "層數不設限：面板 → 面板裡的每一列 → 更深的細項"),
        ],
    ),
    (
        "看懂數字（sysview 的重點）",
        &[
            (
                "e",
                "解釋目前框住的那一格：意義、現在的值、來源、算式、陷阱、原生指令",
            ),
            (
                "",
                "鑽到最裡層可以選到單一數字，例如 Logical CPUs、Core clock",
            ),
            (
                "",
                "也能解釋這台機器上的具體東西：這張 GPU、這個網路介面、這個行程、",
            ),
            ("", "這個掛載點、這顆磁碟、這個使用者、這個路徑"),
            ("", "說明都是寫死的結構化定義加上實測值，不是即時生成的猜測"),
            ("↑ ↓", "說明太長時捲動它（捲到底就停住）"),
            ("其他任意鍵", "關掉說明"),
            ("PgUp PgDn", "在清單裡大幅捲動"),
            ("v", "（CPU 頁）每核顯示切換：逐條列 ⇄ 熱度圖"),
        ],
    ),
    (
        "換頁",
        &[
            ("0", "總覽（` 與 ~ 也可以）"),
            ("1 – 6", "CPU / 記憶體 / GPU / 儲存 / 網路 / 行程"),
            ("A", "管理員擴充功能"),
            ("Tab / Shift-Tab", "依序切換（在任何一層都有效）"),
        ],
    ),
    (
        "通用",
        &[
            ("空白", "暫停 / 繼續取樣"),
            ("+ / -", "加快 / 放慢更新頻率（0.2s – 10s）"),
            ("t", "切換主題"),
            ("m", "切換吉祥物：狐狸 → 鹿 → 關閉"),
            ("? / h", "開關本說明"),
            // 頁尾不列這一行 —— 它是彩蛋，找得到的人才玩得到。
            // 但也不能只有原作者知道，不然那不是彩蛋是私藏；
            // 而且要排在 `q 離開` **前面**，因為那是最後一行，
            // 內容一長就第一個被截掉（真的發生過）。
            ("g", "恐龍跑酷：看數字看累了就來一場"),
            ("q", "離開"),
        ],
    ),
    (
        "行程頁面",
        &[
            ("s", "切換排序欄位"),
            ("/", "關鍵字篩選（Esc 清除）"),
            ("Enter", "鑽進行程表，↑↓ 逐一選取單一行程"),
        ],
    ),
    (
        "管理員功能",
        &[
            (
                "u",
                "透過 sudo 解鎖（sysview 本身永遠不是 root，也不碰你的密碼）",
            ),
            ("Enter", "進入分頁列 → 再 Enter 進入使用者清單"),
            (
                "← →",
                "在分頁列上切換 User Storage / Memory / GPU / Sockets",
            ),
            ("", "分頁排成一列，所以在分頁列上 ↑↓ 不動；↓ 是往裡走一層"),
            ("↑ ↓", "在清單上逐一選取使用者"),
            ("Enter", "展開游標所在使用者的儲存明細"),
            ("[ ]", "不進入分頁列也能直接換分頁（← → 的捷徑）"),
            ("r", "重新查詢"),
        ],
    ),
];

pub struct Help<'a> {
    pub theme: &'a Theme,
    pub version: &'a str,
    /// 品牌與吉祥物。說明頁是**唯一保證看得到吉祥物**的地方 ——
    /// 其他槽都要看終端機夠不夠大，這裡按一個鍵就到。
    pub brand: Option<&'a crate::ui::visual::logo::Brand>,
    pub mascot: Option<(
        crate::ui::visual::mascot::Species,
        crate::ui::visual::mascot::State,
    )>,
    pub frame: u64,
    pub deco: crate::ui::visual::Decoration,
    /// 真的畫出吉祥物時回報。說明頁在窄終端不畫，動畫時鐘就該跟著停。
    pub drawn: Option<&'a std::cell::Cell<bool>>,
    /// 捲到第幾行。
    pub scroll: u16,
    /// 實際能捲到哪裡，畫完之後回寫。
    pub max_scroll_out: Option<&'a std::cell::Cell<u16>>,
}

/// 這些行在指定寬度下會折成幾列。
///
/// `Paragraph` 自己折行但不告訴你折了幾行，而「捲不捲得動」正好需要那個數字。
fn wrapped_len(lines: &[Line<'_>], width: usize) -> u16 {
    if width == 0 {
        return 0;
    }
    let mut n = 0usize;
    for l in lines {
        let w: usize = l
            .spans
            .iter()
            .map(|s| crate::ui::format::width(&s.content))
            .sum();
        n += (w.max(1)).div_ceil(width);
    }
    n.min(u16::MAX as usize) as u16
}

impl Widget for Help<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // 畫面夠寬時右邊讓出一塊給吉祥物，鍵位表不會因此變窄
        use crate::ui::visual::{slot, Decoration};
        let want_art = self.deco.at_least(Decoration::Standard)
            && self.mascot.is_some()
            // 鍵位那一欄要 92 欄才不會把說明文字折斷；擠不下就不畫吉祥物，
            // 說明頁的本業是講清楚按鍵，裝飾排在後面。
            && area.width >= 92 + slot::MASCOT_W + 6
            && area.height >= slot::MASCOT_H + 8;
        if want_art {
            self.render_with_mascot(area, buf);
            return;
        }
        let lines = self.key_lines();
        // 視窗高度照內容算，不寫死。寫死 32 列的結果是排在最後的按鍵
        // 在任何尺寸下都看不到 —— 而說明頁的本業就是把按鍵講清楚。
        let want = wrapped_len(&lines, 74) + 4;
        let win = layout::centered(area, 78, want.min(area.height.saturating_sub(2)));
        self.render_frame(buf, win, lines, None);
    }
}

impl Help<'_> {
    /// 說明頁的外框 + 鍵位表。回傳整個內容區（含右邊給吉祥物的那一塊）。
    ///
    /// 兩種版面（有沒有吉祥物）共用這一段，所以捲動、提示、截斷規則只有
    /// 一份 —— 原本各寫各的，於是「內容超出視窗」在兩邊都沒有處理，
    /// 排在最後的按鍵直接消失。
    fn render_frame(
        &self,
        buf: &mut Buffer,
        win: Rect,
        lines: Vec<Line<'_>>,
        reserve_right: Option<u16>,
    ) -> Rect {
        use crate::ui::widgets::modal::{self, Cap};

        // 先用「扣掉提示列」的高度判斷捲不捲得動，提示才會說實話
        let probe_h = win.height.saturating_sub(4);
        let text_w = win
            .width
            .saturating_sub(4 + reserve_right.unwrap_or(0))
            .max(1);
        let total = wrapped_len(&lines, text_w as usize);
        let scrolls = total > probe_h;
        let caps: &[Cap] = if scrolls {
            &[Cap::Scroll, Cap::CloseAny]
        } else {
            &[Cap::CloseAny]
        };
        let inner = modal::frame(
            buf,
            self.theme,
            win,
            &modal::Modal {
                title: "sysview — 操作說明",
                subtitle: Some(format!("v{}", self.version)),
                border: None,
                caps,
            },
        );
        if inner.width == 0 || inner.height == 0 {
            return inner;
        }
        let keys_w = inner.width.saturating_sub(reserve_right.unwrap_or(0));
        let max = wrapped_len(&lines, keys_w.max(1) as usize).saturating_sub(inner.height);
        if let Some(c) = self.max_scroll_out {
            c.set(max);
        }
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll.min(max), 0))
            .render(
                Rect {
                    width: keys_w,
                    ..inner
                },
                buf,
            );
        inner
    }

    /// 鍵位表在左、品牌與吉祥物在右。
    ///
    /// 這是整個介面裡唯一保證看得到吉祥物的地方：其他槽都要看終端機
    /// 夠不夠大，而說明頁按 `?` 就到，而且它本來就沒有監控資料要讓位。
    fn render_with_mascot(&self, area: Rect, buf: &mut Buffer) {
        use crate::ui::visual::{logo, motif, slot};

        let t = self.theme;
        let total_w = (92 + slot::MASCOT_W + 6).min(area.width);
        let lines = self.key_lines();
        // 至少要放得下吉祥物，其餘照鍵位表的長度來
        // 用**實際**的鍵位欄寬去算，不然「要不要捲」跟真的畫出來的
        // 結果會差一兩行，提示就會在放得下的時候還說「↑↓ 捲動」。
        let keys_w = total_w.saturating_sub(4 + slot::MASCOT_W + 2);
        let want = (wrapped_len(&lines, keys_w.max(1) as usize) + 4).max(slot::MASCOT_H + 10);
        let win = layout::centered(area, total_w, want.min(area.height.saturating_sub(2)));
        let inner = self.render_frame(buf, win, lines, Some(slot::MASCOT_W + 2));
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let art_w = slot::MASCOT_W + 2;
        let art = Rect {
            x: inner.x + inner.width.saturating_sub(art_w),
            width: art_w,
            ..inner
        };

        // 品牌
        let mut y = art.y;
        if let Some(brand) = self.brand {
            let used = slot::logo(buf, Rect { height: 4, ..art }, t, brand, self.deco);
            y += used;
            if y < art.y + art.height {
                buf.set_string(
                    art.x,
                    y,
                    crate::ui::format::truncate(&logo::tagline(brand), art.width as usize),
                    t.faint_style(),
                );
                y += 1;
            }
        }
        if y < art.y + art.height {
            buf.set_string(art.x, y, motif::rule(art.width as usize), t.border_style());
            y += 1;
        }

        if let Some((species, state)) = self.mascot {
            let left = (art.y + art.height).saturating_sub(y);
            slot::mascot(
                buf,
                Rect {
                    x: art.x,
                    y,
                    width: art.width,
                    height: left,
                },
                t,
                slot::Draw {
                    species,
                    state,
                    frame: self.frame,
                    deco: self.deco,
                    captioned: left >= slot::MASCOT_H + 4,
                    drawn: self.drawn,
                },
            );
        }
    }

    /// 鍵位表的內容。抽出來讓兩種版面共用。
    fn key_lines(&self) -> Vec<Line<'_>> {
        let head = self.theme.bold(self.theme.palette.accent);
        let key = self.theme.bold(self.theme.palette.fg);
        let desc = self.theme.dim_style();
        let mut lines: Vec<Line> = Vec::new();
        for (title, entries) in SECTIONS {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(*title, head)));
            for (k, d) in *entries {
                lines.push(Line::from(vec![
                    Span::styled("  ", desc),
                    Span::styled(crate::ui::format::pad(k, 16), key),
                    Span::styled(*d, desc),
                ]));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "資料全部來自 /proc 與 /sys，不需要 root。NVIDIA 走 NVML。",
            self.theme.faint_style(),
        )));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorDepth;

    #[test]
    fn documents_every_important_key() {
        let all: String = SECTIONS
            .iter()
            .flat_map(|(_, e)| e.iter().map(|(k, _)| *k))
            .collect::<Vec<_>>()
            .join(" ");
        for k in ["e", "u", "s", "/", "t", "q", "Esc", "空白", "A", "Enter"] {
            assert!(all.contains(k), "說明缺少 {k:?} 的鍵位");
        }
    }

    #[test]
    fn explain_key_is_documented_prominently() {
        // `e` 是本工具的核心差異功能，必須在說明裡講清楚
        let text: String = SECTIONS
            .iter()
            .flat_map(|(t, e)| {
                std::iter::once((*t).to_owned()).chain(e.iter().map(|(k, d)| format!("{k} {d}")))
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("解釋"), "說明應強調 Explain 功能");
        assert!(text.contains("原生指令"));
        // 多層導航是這一版最容易搞不懂的地方，說明必須講清楚
        assert!(text.contains("上一層"), "說明必須交代怎麼退回上一層");
        assert!(text.contains("層數不設限"), "說明必須交代層數沒有上限");
        // Describe 不是 LLM 即時生成的，這一點要講明白，使用者才敢信
        assert!(
            text.contains("不是即時生成"),
            "說明必須交代解釋內容不是即時生成的"
        );
    }

    #[test]
    fn admin_navigation_is_documented_without_relying_on_brackets() {
        let admin = SECTIONS
            .iter()
            .find(|(t, _)| *t == "管理員功能")
            .expect("應該有管理員功能區塊");
        let text: String = admin
            .1
            .iter()
            .map(|(k, d)| format!("{k} {d}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("分頁列"), "要交代 Enter 會先進到分頁列");
        assert!(text.contains("選取使用者"), "要交代 ↑↓ 是用來選使用者的");
        // `[` `]` 現在只是捷徑，不該被寫成唯一的切換方式
        assert!(
            text.contains("捷徑"),
            "[ ] 應該被描述成捷徑，而不是必要的操作"
        );
    }

    #[test]
    fn renders_in_small_terminals() {
        let t = Theme::new("default", ColorDepth::TrueColor);
        for (w, h) in [(50u16, 14u16), (80, 24), (200, 60)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
            Help {
                theme: &t,
                version: "2.0.0",
                brand: None,
                mascot: None,
                frame: 0,
                deco: crate::ui::visual::Decoration::None,
                drawn: None,
                scroll: 0,
                max_scroll_out: None,
            }
            .render(Rect::new(0, 0, w, h), &mut buf);
        }
    }
}
