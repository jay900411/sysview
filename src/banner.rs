//! 登入時的歡迎畫面：吉祥物在左、logo 與一句邀請在右。
//!
//! 這不是 TUI 的一部分，是 `sysview --banner` 印出來的一段純文字，安裝時
//! （`make install-motd`）用建置者的身分產成檔案，登入腳本只 `cat` 它 ——
//! 登入時 PAM 以 root 執行 motd 腳本，所以那時**不能**跑任何 sysview 程式碼，
//! 也因此畫面上沒有任何即時數字：要看數字，打 `sysview`。
//!
//! 寬度壓在 80 欄以內（ssh 客戶端的最小公分母），十二列。顏色只用 256 色的
//! SGR，`--no-color` / `NO_COLOR` 就完全不帶跳脫序列。

use crate::ui::format;
use crate::ui::visual::logo::{self, Brand};
use crate::ui::visual::mascot::{self, Species, State};
use crate::ui::visual::Decoration;

/// 最寬幾欄。
pub const WIDTH: usize = 80;
/// 左邊留白。
const MARGIN: usize = 2;
/// 吉祥物與文字之間。
const GAP: usize = 3;
/// 吉祥物露幾列：頭與肩膀從畫面底部探出來，跟儀表板裡的水印一樣，
/// 不是整隻 —— 登入畫面不該佔掉半個螢幕。
const MASCOT_ROWS: u16 = 8;

/// 右欄的字。`{sysview}` 與 `{e}` 會被換成粗體。
const PITCH: [&str; 6] = [
    "Use {sysview} to watch CPU, memory, GPU,",
    "storage, network and processes, all in one.",
    "No root needed. Press {e} on any number to",
    "see what it means and where it comes from.",
    "",
    "{$} {sysview}",
];

/// 產生整段文字（含換行）。`colour` 為 false 時沒有任何跳脫序列。
pub fn banner(brand: &Brand, colour: bool) -> String {
    let (accent, fur, dim, bold, off) = if colour {
        (
            "\x1b[38;5;75m",
            "\x1b[38;5;214m",
            "\x1b[38;5;245m",
            "\x1b[1m",
            "\x1b[0m",
        )
    } else {
        ("", "", "", "", "")
    };

    let fox = mascot::reveal(Species::Fox, State::Observe, 0, MASCOT_ROWS);
    let fox_w = usize::from(fox.w);
    let column = WIDTH.saturating_sub(MARGIN + fox_w + GAP);

    // 右欄：logo（放不下點陣就退回文字，跟 TUI 同一套判斷）、標語、邀請
    let mut right: Vec<String> = brand
        .render(column, 4, Decoration::Full)
        .into_iter()
        .map(|l| format!("{accent}{}{off}", l.trim_end()))
        .collect();
    right.push(format!("{dim}{}{off}", logo::slogan(brand)));
    right.push(String::new());
    right.extend(PITCH.iter().map(|l| {
        l.replace("{sysview}", &format!("{bold}sysview{off}"))
            .replace("{e}", &format!("{bold}e{off}"))
            .replace("{$}", &format!("{dim}${off}"))
    }));

    // 吉祥物貼底：牠是從下面探出來的
    let rows = right.len().max(usize::from(fox.h));
    let fox_top = rows - usize::from(fox.h);
    let mut out = String::new();
    for i in 0..rows {
        let art = i
            .checked_sub(fox_top)
            .and_then(|k| fox.lines.get(k))
            .map(String::as_str)
            .unwrap_or("");
        let msg = right.get(i).map(String::as_str).unwrap_or("");
        // 上色的字串尾巴是跳脫序列，trim 不到裡面的空白：先把圖自己的尾巴修掉
        let art = art.trim_end();
        let mut line = " ".repeat(MARGIN);
        if !art.is_empty() {
            line.push_str(fur);
            line.push_str(art);
            line.push_str(off);
        }
        if !msg.is_empty() {
            line.push_str(&" ".repeat(fox_w - format::width(art) + GAP));
            line.push_str(msg);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// 去掉 SGR，量寬度用。
pub fn strip_sgr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_brand() -> Brand {
        Brand::new("", "auto")
    }

    #[test]
    fn the_banner_fits_eighty_columns_and_says_the_essentials() {
        let text = banner(&default_brand(), false);
        assert!(!text.contains('\x1b'));
        assert!(text.contains("Use sysview to"));
        assert!(text.contains("all in one"));
        assert!(text.contains("one command, the whole machine"));
        assert!(text.contains("$ sysview"));
        let lines: Vec<&str> = text.lines().collect();
        assert!((10..=24).contains(&lines.len()), "{} 列", lines.len());
        for l in &lines {
            assert!(format::width(l) <= WIDTH, "超過 80 欄：{l:?}");
            assert_eq!(l.trim_end(), *l, "行尾不留空白");
        }
        // 點陣 logo 與吉祥物都真的在
        assert!(text.contains("▄▀▀▀▀ █   █"), "logo 第一列");
        assert!(text.contains("▙▖▗▀▖"), "狐狸的耳朵");
    }

    #[test]
    fn colour_only_adds_sgr_and_changes_no_character() {
        let plain = banner(&default_brand(), false);
        let colour = banner(&default_brand(), true);
        assert!(colour.contains("\x1b[38;5;75m"), "logo 有上色");
        assert!(colour.contains("\x1b[38;5;214m"), "狐狸有上色");
        assert!(colour.contains("\x1b[1msysview\x1b[0m"), "指令粗體");
        let stripped: String = colour.lines().map(|l| strip_sgr(l) + "\n").collect();
        assert_eq!(stripped, plain, "上色不能改變任何字");
        for l in colour.lines() {
            assert!(format::width(&strip_sgr(l)) <= WIDTH, "超過 80 欄：{l:?}");
        }
    }

    #[test]
    fn a_custom_brand_and_a_too_long_one_both_render() {
        let t = banner(&Brand::new("DEVLAB", "auto"), false);
        assert!(t.contains("sysview observability"), "自訂品牌用通用標語");
        assert!(t.contains("Use sysview to"), "邀請還是講 sysview");
        // 代號經過驗證最多 6 字，但更長也不能炸掉或超寬
        let t = banner(
            &Brand::new("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789ABCDEF", "auto"),
            false,
        );
        for l in t.lines() {
            assert!(format::width(l) <= WIDTH, "超過 80 欄：{l:?}");
        }
    }
}
