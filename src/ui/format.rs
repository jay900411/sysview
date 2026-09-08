//! 數值格式化與終端機寬度處理。
//!
//! 中文字在終端機佔 **2 格**。整個 UI 的欄寬計算都必須用顯示寬度，
//! 不能用 `str::len()`（那是位元組數）也不能用 `chars().count()`（那是碼位數）。
//! v1 就是因為用了 `strlen()` 導致所有框線歪掉。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::metrics::model::{Quality, Reading, Unit};

/// 字串在終端機上佔幾格。
pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// 裁切到最多 `max` 格寬。被裁到就補上省略號。
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width(s) <= max {
        return s.to_owned();
    }
    if max == 1 {
        return "…".into();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > max - 1 {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.push('…');
    out
}

/// 補空白到剛好 `w` 格寬（靠左）。
pub fn pad(s: &str, w: usize) -> String {
    let t = truncate(s, w);
    let cur = width(&t);
    format!("{t}{}", " ".repeat(w.saturating_sub(cur)))
}

/// 補空白到剛好 `w` 格寬（靠右）。
pub fn pad_left(s: &str, w: usize) -> String {
    let t = truncate(s, w);
    let cur = width(&t);
    format!("{}{t}", " ".repeat(w.saturating_sub(cur)))
}

/// 位元組 → 人類可讀。用 1024 進位（與 `free -h`、`df -h` 一致）。
pub fn bytes(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let neg = v < 0.0;
    let mut n = v.abs();
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut i = 0;
    while n >= 1024.0 && i < UNITS.len() - 1 {
        n /= 1024.0;
        i += 1;
    }
    let s = if i == 0 {
        format!("{n:.0} B")
    } else if n < 10.0 {
        format!("{n:.2} {}", UNITS[i])
    } else if n < 100.0 {
        format!("{n:.1} {}", UNITS[i])
    } else {
        format!("{n:.0} {}", UNITS[i])
    };
    if neg {
        format!("-{s}")
    } else {
        s
    }
}

/// 每秒位元組數。
pub fn rate(v: f64) -> String {
    format!("{}/s", bytes(v))
}

/// 頻率。
pub fn hertz(mhz: f64) -> String {
    if !mhz.is_finite() {
        return "—".into();
    }
    if mhz >= 1000.0 {
        format!("{:.2} GHz", mhz / 1000.0)
    } else {
        format!("{mhz:.0} MHz")
    }
}

/// 秒數 → `21d 01:23` 或 `01:23:45`。
pub fn duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "—".into();
    }
    let t = secs as u64;
    let (d, r) = (t / 86400, t % 86400);
    let (h, r) = (r / 3600, r % 3600);
    let (m, s) = (r / 60, r % 60);
    if d > 0 {
        format!("{d}d {h:02}:{m:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
    }
}

/// 大數字加上千位分隔，方便閱讀。
pub fn count(v: f64) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    if v >= 1_000_000.0 {
        format!("{:.1}M", v / 1e6)
    } else if v >= 1000.0 {
        format!("{:.1}k", v / 1e3)
    } else {
        format!("{v:.0}")
    }
}

/// 把 [`Reading`] 格式化成顯示字串。
///
/// **拿不到就明白說拿不到**，絕不用 0 冒充。估計值會加上 `~` 前綴。
pub fn reading(r: &Reading) -> String {
    match (&r.quality, r.value) {
        (Quality::Unavailable(u), _) => u.short().to_owned(),
        (_, None) => "n/a".into(),
        (q, Some(v)) => {
            let body = match r.unit {
                Unit::Percent => format!("{v:.1}%"),
                Unit::Bytes => bytes(v),
                Unit::BytesPerSec => rate(v),
                Unit::Celsius => format!("{v:.0}°C"),
                Unit::Watts => format!("{v:.1} W"),
                Unit::Megahertz => hertz(v),
                Unit::Count => count(v),
                Unit::CountPerSec => format!("{}/s", count(v)),
                Unit::Seconds => duration(v),
                Unit::Scalar => format!("{v:.2}"),
            };
            if q.is_estimated() {
                format!("~{body}")
            } else {
                body
            }
        }
    }
}

/// 給欄位用的固定寬度版本。
pub fn reading_padded(r: &Reading, w: usize) -> String {
    pad_left(&reading(r), w)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Unavailable;

    #[test]
    fn cjk_characters_count_as_two_columns() {
        assert_eq!(width("記憶體"), 6, "中文一個字佔 2 格");
        assert_eq!(width("abc"), 3);
        assert_eq!(width("CPU 使用率"), 4 + 6);
        // 點陣圖字元是半形
        assert_eq!(width("⣿⣿"), 2, "braille 是半形，用於圖表");
    }

    #[test]
    fn truncate_respects_display_width_not_byte_length() {
        assert_eq!(truncate("記憶體使用率", 6), "記憶…");
        // 不變量是「不超過」而非「剛好等於」—— 2 格寬的中文塞不進剩下的 1 格，
        // 這時寧可少一格也不能撐破欄位。
        assert!(width(&truncate("記憶體使用率", 6)) <= 6);
        assert_eq!(truncate("abcdefgh", 5), "abcd…");
        assert_eq!(width(&truncate("abcdefgh", 5)), 5, "半形字應能剛好填滿");
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate("", 5), "");
        assert_eq!(truncate("abcdef", 0), "");
    }

    #[test]
    fn truncate_never_exceeds_budget_for_any_input() {
        for s in ["記憶體", "abc記憶體def", "🎉🎉🎉", "a", ""] {
            for max in 0..12 {
                let t = truncate(s, max);
                assert!(
                    width(&t) <= max,
                    "{s:?} 裁到 {max} 格卻變成 {} 格",
                    width(&t)
                );
            }
        }
    }

    #[test]
    fn pad_produces_exact_width_with_cjk() {
        for s in ["記憶體", "abc", "GPU 顯示卡", ""] {
            assert_eq!(width(&pad(s, 20)), 20, "{s:?} 補齊後寬度不對");
            assert_eq!(width(&pad_left(s, 20)), 20);
        }
    }

    #[test]
    fn formats_bytes_with_binary_prefixes() {
        assert_eq!(bytes(0.0), "0 B");
        assert_eq!(bytes(1023.0), "1023 B");
        assert_eq!(bytes(1024.0), "1.00 KB");
        assert_eq!(bytes(1536.0), "1.50 KB");
        assert_eq!(bytes(65_536_000_000.0), "61.0 GB");
        assert_eq!(bytes(-1024.0), "-1.00 KB");
    }

    #[test]
    fn formats_non_finite_safely() {
        assert_eq!(bytes(f64::NAN), "—");
        assert_eq!(bytes(f64::INFINITY), "—");
        assert_eq!(hertz(f64::NAN), "—");
        assert_eq!(duration(f64::NAN), "—");
        assert_eq!(duration(-5.0), "—");
        assert_eq!(count(f64::NEG_INFINITY), "—");
    }

    #[test]
    fn formats_frequency_and_duration() {
        assert_eq!(hertz(800.0), "800 MHz");
        assert_eq!(hertz(4770.0), "4.77 GHz");
        assert_eq!(duration(3661.0), "01:01:01");
        assert_eq!(duration(21.0 * 86400.0 + 3600.0), "21d 01:00");
    }

    #[test]
    fn unavailable_readings_never_show_zero() {
        let r = Reading::unavailable(Unit::Percent, Unavailable::PermissionDenied);
        let s = reading(&r);
        assert_eq!(s, "permission required");
        assert!(!s.contains('0'), "拿不到的值絕不能顯示成 0");

        let r = Reading::unsupported(Unit::Celsius);
        assert_eq!(reading(&r), "unsupported");
    }

    #[test]
    fn estimated_readings_are_visibly_marked() {
        let r = Reading::estimated(75.0, Unit::Percent, "RC6 residency");
        assert_eq!(reading(&r), "~75.0%", "估計值必須有可見標記");
        let exact = Reading::exact(75.0, Unit::Percent);
        assert_eq!(reading(&exact), "75.0%");
    }

    #[test]
    fn every_unit_formats_without_panic() {
        for unit in [
            Unit::Percent,
            Unit::Bytes,
            Unit::BytesPerSec,
            Unit::Celsius,
            Unit::Watts,
            Unit::Megahertz,
            Unit::Count,
            Unit::CountPerSec,
            Unit::Seconds,
            Unit::Scalar,
        ] {
            let s = reading(&Reading::exact(42.0, unit));
            assert!(!s.is_empty(), "{unit:?} 格式化結果不該是空的");
        }
    }
}
