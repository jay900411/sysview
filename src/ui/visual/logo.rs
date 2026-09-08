//! Logo / branding rendering。
//!
//! 品牌代號只是識別，不是海報。所以這裡的每一種呈現方式都有明確的
//! 空間預算，畫不下就往下降一級，降到最後就只剩一行文字。
//!
//! 降級順序：`Pixel → Small → Text → 什麼都不畫`。

use crate::ui::visual::glyph;
use crate::ui::visual::Decoration;

/// logo 的呈現方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// 完整點陣（4 個字元列高）。
    Pixel,
    /// 半高點陣：只取點陣的中間三列壓成兩列，用在頁首。
    Small,
    /// 純文字：`// DEVLAB`。
    Text,
    /// 不畫。
    Off,
}

/// 品牌識別。從 config 算出來一次，之後畫面上到處用。
#[derive(Debug, Clone, PartialEq)]
pub struct Brand {
    /// 使用者設的代號，空字串代表用 sysview 預設品牌。
    pub name: String,
    /// 使用者要求的呈現方式。
    pub want: String,
}

impl Brand {
    pub fn new(name: &str, want: &str) -> Self {
        Self {
            name: name.to_owned(),
            want: want.to_owned(),
        }
    }

    /// 畫面上要顯示的字。
    ///
    /// 沒設代號就是產品自己的名字，而且維持小寫 —— `SYSVIEW` 讀起來像在喊。
    /// 自訂代號則一律大寫，因為那是識別碼不是單字（驗證時就轉好了）。
    pub fn word(&self) -> &str {
        if self.name.is_empty() {
            "sysview"
        } else {
            &self.name
        }
    }

    /// 使用者有沒有自己設過品牌。
    pub fn is_custom(&self) -> bool {
        !self.name.is_empty()
    }

    /// 在給定的空間與裝飾層級下，實際要用哪一種呈現。
    ///
    /// `width` / `height` 是可用的欄與列。
    pub fn style(&self, width: usize, height: usize, deco: Decoration) -> Style {
        match self.want.as_str() {
            "off" => return Style::Off,
            "text" => return Style::Text,
            _ => {}
        }
        let w = self.word();
        let pixel_w = pixel_width(w);
        let forced = self.want == "pixel" || self.want == "small";

        if deco == Decoration::None && !forced {
            return Style::Text;
        }
        if self.want == "pixel" {
            // 明確指定 pixel 也還是要放得下，放不下就降級 —— 版面不能爆
            return if pixel_w <= width && height >= 4 {
                Style::Pixel
            } else if pixel_w <= width && height >= 2 {
                Style::Small
            } else {
                Style::Text
            };
        }
        if self.want == "small" {
            return if pixel_w <= width && height >= 2 {
                Style::Small
            } else {
                Style::Text
            };
        }
        // auto
        if deco.at_least(Decoration::Full) && pixel_w <= width && height >= 4 {
            Style::Pixel
        } else if deco.at_least(Decoration::Standard) && pixel_w <= width && height >= 2 {
            Style::Small
        } else {
            Style::Text
        }
    }

    /// 畫出來。回傳每一列的字串，呼叫端負責上色與定位。
    ///
    /// 回傳空 vec 代表這個尺寸下不畫 logo。
    pub fn render(&self, width: usize, height: usize, deco: Decoration) -> Vec<String> {
        let word = self.word();
        match self.style(width, height, deco) {
            Style::Off => Vec::new(),
            Style::Text => {
                let t = format!("// {word}");
                if crate::ui::format::width(&t) <= width {
                    vec![t]
                } else if word.len() <= width {
                    vec![word.to_owned()]
                } else {
                    Vec::new()
                }
            }
            Style::Small => small(word),
            Style::Pixel => match glyph::layout(word, 1) {
                Some(rows) => glyph::to_half_blocks(&rows),
                // 理論上不會發生（代號已經過驗證），但不能因此畫出空白
                None => vec![format!("// {word}")],
            },
        }
    }
}

/// 點陣模式需要幾欄。
pub fn pixel_width(word: &str) -> usize {
    let n = word.chars().count();
    if n == 0 {
        0
    } else {
        n * glyph::GLYPH_W + (n - 1)
    }
}

/// 半高點陣：把 7 列壓成 4 列，再用半格畫成兩列。
///
/// **不是**取中間四列。原本的做法是 `rows[1..5]` —— 那會把每個字的頂線
/// 與底線整段丟掉，`C` 變成兩根豎線、`S` 變成一堆橫槓，看起來像 logo
/// 被上下腰斬。壓縮的正確做法是把相鄰兩列**疊起來**（OR）：筆畫會變粗，
/// 但一筆都不會消失。
fn small(word: &str) -> Vec<String> {
    let Some(rows) = glyph::layout(word, 1) else {
        return vec![format!("// {word}")];
    };
    glyph::to_half_blocks(&squash(&rows))
}

/// 把 7 列點陣疊成 4 列：(0,1) (2,3) (4,5) (6)。
fn squash(rows: &[String]) -> Vec<String> {
    let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    let lit =
        |r: Option<&String>, x: usize| r.and_then(|r| r.chars().nth(x)).is_some_and(|c| c == '#');
    let mut out = Vec::with_capacity(4);
    for pair in rows.chunks(2) {
        out.push(
            (0..w)
                .map(|x| {
                    if lit(pair.first(), x) || lit(pair.get(1), x) {
                        '#'
                    } else {
                        ' '
                    }
                })
                .collect(),
        );
    }
    out
}

/// 只有標語那一段，給 logo 正下方用。
///
/// logo 已經把品牌代號放大寫在上面了，下一行再來一次 `// DEVLAB  |  …`
/// 就是同一個字連寫兩遍。標語本身照 concept 圖：預設品牌是那句
/// "one command, the whole machine"，自訂品牌則說明這是 sysview。
pub fn slogan(brand: &Brand) -> &'static str {
    if brand.is_custom() {
        "sysview observability"
    } else {
        "one command, the whole machine"
    }
}

/// 給品牌用的一行式標語列（concept 圖的頁尾語氣）。
pub fn tagline(brand: &Brand) -> String {
    if brand.is_custom() {
        format!("// {}  |  sysview observability", brand.word())
    } else {
        "// SYSVIEW  |  one command, the whole machine".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_brand_is_sysview() {
        let b = Brand::new("", "auto");
        assert_eq!(b.word(), "sysview");
        assert!(!b.is_custom());
    }

    #[test]
    fn custom_brand_replaces_the_word_but_not_the_product() {
        let b = Brand::new("DEVLAB", "auto");
        assert_eq!(b.word(), "DEVLAB");
        assert!(b.is_custom());
        // 標語仍然要講得出這是 sysview，不然換了品牌就沒人知道這是什麼工具
        assert!(tagline(&b).contains("sysview"));
    }

    #[test]
    fn style_steps_down_as_space_runs_out() {
        let b = Brand::new("DEVLAB", "auto");
        let w = pixel_width("DEVLAB");
        assert_eq!(b.style(w, 6, Decoration::Full), Style::Pixel);
        assert_eq!(b.style(w, 2, Decoration::Full), Style::Small);
        assert_eq!(b.style(w, 6, Decoration::Standard), Style::Small);
        assert_eq!(b.style(w - 1, 6, Decoration::Full), Style::Text);
        assert_eq!(b.style(w, 6, Decoration::None), Style::Text);
    }

    #[test]
    fn explicitly_requested_pixel_still_backs_off_when_it_does_not_fit() {
        // 使用者要 pixel，但畫面放不下 —— 寧可降級也不要撐破版面
        let b = Brand::new("DEVLAB", "pixel");
        assert_eq!(b.style(10, 6, Decoration::Full), Style::Text);
        assert_eq!(
            b.style(pixel_width("DEVLAB"), 6, Decoration::None),
            Style::Pixel
        );
    }

    #[test]
    fn off_never_renders_anything() {
        let b = Brand::new("DEVLAB", "off");
        assert!(b.render(200, 20, Decoration::Full).is_empty());
    }

    #[test]
    fn rendered_lines_never_exceed_the_given_width() {
        for word in ["A", "DEVLAB", "SYSVIEW", "X1"] {
            for want in ["auto", "pixel", "small", "text"] {
                let b = Brand::new(if word == "SYSVIEW" { "" } else { word }, want);
                for width in [4usize, 8, 20, 40, 80] {
                    for deco in [
                        Decoration::None,
                        Decoration::Minimal,
                        Decoration::Standard,
                        Decoration::Full,
                    ] {
                        for line in b.render(width, 6, deco) {
                            assert!(
                                crate::ui::format::width(&line) <= width,
                                "{want}/{word} 在 {width} 欄畫出了 {} 欄的東西：{line:?}",
                                crate::ui::format::width(&line)
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pixel_width_matches_what_layout_actually_produces() {
        for word in ["A", "AB", "DEVLAB", "SYSVIEW"] {
            let rows = glyph::layout(word, 1).unwrap();
            assert_eq!(rows[0].chars().count(), pixel_width(word), "{word}");
        }
    }
}
