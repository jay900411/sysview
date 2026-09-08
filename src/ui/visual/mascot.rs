//! Mascot system —— 狐狸與鹿。
//!
//! # 這是什麼
//!
//! 一組 terminal-native 的像素剪影，用上下半格（`▀▄█`）畫在字元格裡。
//! 圖是從 concept 圖直接萃取的，保留了原作者畫的線條與細節。
//! 不依賴任何圖片協定（sixel / kitty / iterm），所以 SSH 進到哪台機器
//! 都畫得出來，單色終端也還在。
//!
//! # 狀態
//!
//! 沿用 concept 的語彙：
//!
//! | 狀態 | 意思 | 什麼時候出現 |
//! |---|---|---|
//! | `observe` | 安靜地看著 | 一切正常 |
//! | `explore` | 走動、好奇 | 有負載但健康 |
//! | `proceed` | 低頭專注 | 高負載 |
//! | `rest`    | 趴著休息 | 系統很閒 |
//! | `return`  | 回望 | 剛從警告狀態恢復 |
//!
//! # 動畫怎麼做到不吃 CPU
//!
//! 每個狀態只有**一張**點陣圖，加上幾個很小的 frame delta（清掉幾個點、
//! 點亮幾個點、整體上移一格）。眨眼就是把眼睛那一格關掉一瞬間，
//! 呼吸就是整體上移一像素。這樣：
//!
//! * 不必存多張完整點陣圖，二進位檔不會膨脹
//! * 每格的計算量是「幾個座標」而不是「整張圖」
//! * 動畫時鐘完全獨立於 collector —— 動畫再怎麼跑也不會多讀一次 `/proc`
//!
//! 預設 3 fps。上限 8 fps，而且是硬性的：這是裝飾，不值得為它燒 CPU。

/// 吉祥物種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Species {
    Fox,
    Deer,
}

impl Species {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "fox" => Self::Fox,
            "deer" => Self::Deer,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Fox => "FOX",
            Self::Deer => "DEER",
        }
    }
}

/// 姿態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Observe,
    Explore,
    Proceed,
    Rest,
    Return,
}

pub const STATE_NAMES: &[&str] = &["observe", "explore", "proceed", "rest", "return"];

impl State {
    pub const ALL: &'static [State] = &[
        State::Observe,
        State::Explore,
        State::Proceed,
        State::Rest,
        State::Return,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "observe" => Self::Observe,
            "explore" => Self::Explore,
            "proceed" => Self::Proceed,
            "rest" => Self::Rest,
            "return" => Self::Return,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Observe => "OBSERVE",
            Self::Explore => "EXPLORE",
            Self::Proceed => "PROCEED",
            Self::Rest => "REST",
            Self::Return => "RETURN",
        }
    }

    /// 兩行標語，沿用 concept 的語氣。
    pub fn caption(self) -> (&'static str, &'static str) {
        match self {
            Self::Observe => ("QUIET AWARENESS", "EVERYTHING LEAVES A TRACE."),
            Self::Explore => ("STEADY CURIOSITY", "FURTHER IS A KIND OF HOME."),
            Self::Proceed => ("FOCUSED MOVEMENT", "SMALL STEPS. BIGGER PATHS."),
            Self::Rest => ("STILL PRESENCE", "A CALMER MIND EXPLORES MORE."),
            Self::Return => ("ALWAYS A WAY BACK", "SOME PLACES STAY WITH YOU."),
        }
    }
}

/// 一格動畫要對基礎點陣做的修改。
///
/// 刻意做成「差異」而不是完整的一張圖：眨眼只是關掉四個點，
/// 存一整張新點陣圖來表達那件事太浪費了。
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    /// 背線呼吸：把上緣輪廓**移動**一個子像素 —— 清掉舊的那格、點亮上面那格。
    ///
    /// 早期版本只「多點亮上面那格」而不清掉舊的，那不是移動是加厚：
    /// 畫面上看起來像原本的剪影外面多描了一層邊，一點都不像在呼吸。
    /// 只作用在 [`Pose::back`] 那一段，所以腳底、地面、尾巴根、
    /// 頭部與鹿角都不會動。
    pub breath: bool,
    /// 耳朵微動。用同一套輪廓位移，範圍是 [`Pose::ears`]。
    /// 鹿不做 —— 鹿角抖起來很假。
    pub ears: bool,
    /// 尾巴翹起來：`0` 不動、`1` 甩到一半、`2` 完全勾起。
    ///
    /// 這不是把外緣平移一格 —— 那種幅度小到跟呼吸分不出來（第一版就是
    /// 這樣，看起來只是輪廓在抖）。這裡做的是**改曲率**：尾巴根部不動，
    /// 越靠近尾尖抬得越高，整條弧線從往下垂變成往上勾。
    /// 身體、頭、腳都不在 [`Pose::tail`] 的範圍裡，所以只有尾巴會動。
    pub tail: i8,
    /// 這一格要關掉的點（眨眼）。
    pub clear: &'static [(u8, u8)],
    /// 這一格要點亮的點。
    pub set: &'static [(u8, u8)],
}

const STILL: Frame = Frame {
    breath: false,
    ears: false,
    tail: 0,
    clear: &[],
    set: &[],
};
const BREATH: Frame = Frame {
    breath: true,
    ears: false,
    tail: 0,
    clear: &[],
    set: &[],
};
const EARS: Frame = Frame {
    breath: false,
    ears: true,
    tail: 0,
    clear: &[],
    set: &[],
};
/// 尾巴甩到一半。用在勾起來與放下的前後各一格 —— 直接從平的跳到勾起
/// 會看起來像換了一張圖，中間補一格就變成一個動作。
const TAIL_MID: Frame = Frame {
    breath: false,
    ears: false,
    tail: 1,
    clear: &[],
    set: &[],
};
/// 尾巴完全勾起來。
///
/// 用法是「勾起來、停一下、放下」，不是左右來回 —— 來回切換看起來像
/// 節拍器，而坐著的動物偶爾把尾巴一勾、停幾秒、再放下，才像活的。
const TAIL_UP: Frame = Frame {
    breath: false,
    ears: false,
    tail: 2,
    clear: &[],
    set: &[],
};

/// 尾巴抬起來的**形狀**：每一欄抬多少，是它離根部的距離 `t`（0 = 根部、
/// 1 = 尾尖）的函數，回傳「佔最大抬升的幾成」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    /// 弧：根部不動，越靠尾尖抬得越多（t²）。
    ///
    /// 趴著的姿態尾巴貼地，只有尾尖翹起來；動根部會很假。
    Arc,
    /// 波：根部就開始往上轉、中段漸緩、過了弧頂尾尖再垂下來 ——
    /// 一個反過來的 S。
    ///
    /// 站著的姿態整條尾巴懸空，甩起來從根部就動；只翹尾尖看起來像一根
    /// 棒子末端在抖。三次式 `1.8t − 0.55t² − 0.5t³`：根部斜率 1.8（比
    /// 平均陡，靠身體那段先動）、中點到七成、弧頂在八成處到八成三、
    /// 尾尖回落到七成半（S 的第二個彎）。
    Wave,
}

impl Curve {
    /// 每一欄抬多少（佔最大抬升的幾成）。
    fn at(self, t: f32) -> f32 {
        match self {
            // 只有外面三分之一真的翹起來（t³）。t² 會讓中段也離地，整條像
            // 一根舉起來的棒子 —— 使用者看了兩次都說怪。
            Curve::Arc => t * t * t,
            Curve::Wave => 1.8 * t - 0.55 * t * t - 0.5 * t * t * t,
        }
    }

    /// 尾尖收到根部粗細的幾成（乘法、只看離根部多遠，smoothstep 接過去）。
    ///
    /// 早期是「抬多少就從下緣收多少」，抬得快的那幾欄一下子從七個子像素
    /// 收到一個：坐姿的尾巴像一截粗的擱在地上、後面接一根鬚。坐著的蓬一點。
    fn thin(self) -> f32 {
        match self {
            Curve::Arc => 0.5,
            Curve::Wave => 0.6,
        }
    }

    /// 根部這一段（`t` 在此之前）的下緣抬得比上緣少、黏著身體。
    ///
    /// 站著的尾巴根部一翹，臀部那個角就露出來成一個尖；下緣慢慢跟上，
    /// 底下才圓過去（「反折的底部太尖」）。坐著的根部本來就不動，不需要。
    fn fillet(self) -> f32 {
        match self {
            Curve::Arc => 0.0,
            Curve::Wave => 0.3,
        }
    }

    /// 每一欄至少抬幾個子像素。
    ///
    /// 站著的整條懸空，連根部都要動 —— 根部釘死，旁邊那一欄一翹就在交界
    /// 留一個缺口。坐著的根部貼地，不動。
    fn floor(self) -> usize {
        match self {
            Curve::Arc => 0,
            Curve::Wave => 1,
        }
    }

    /// 下緣往尾尖走，每一欄最多往下幾個子像素。
    ///
    /// 站著的尾巴抬起來，底下沒有理由往下凸（原圖貼地那段的階梯、尾尖
    /// 垂下來的那一欄都會變成掛在下面的一點）；坐著的貼地那段本來就比
    /// 根部低，准它一格一格降下去。
    fn sag(self) -> usize {
        match self {
            Curve::Arc => 1,
            Curve::Wave => 0,
        }
    }
}

/// 一個姿態的完整定義。
pub struct Pose {
    pub state: State,
    /// 像素列，`#` 代表點亮。所有列等寬。112 × 32 個子像素。
    pub art: &'static [&'static str],
    /// 呼吸時上緣會移動的欄位範圍（含頭尾）。
    ///
    /// 只有背線這一段 —— 挑的是「頭部之前、尾巴之後」那段平滑的下降。
    /// 整隻都動的話腳底會漂、鹿角會抖，那正是要避免的。
    pub back: (u8, u8),
    /// 耳朵微動的欄位範圍。`(0, 0)` 代表這個姿態不做。
    pub ears: (u8, u8),
    /// 尾巴的範圍 `(x0, x1, y0, y1)`，`x1` 是**根部**、`x0` 是**尾尖**。
    /// `(0, 0, 0, 0)` 代表這個姿態不做。
    ///
    /// 只有「安靜」的姿態才給值。負載高的時候還在開心翹尾巴，
    /// 傳達的訊息是錯的 —— 吉祥物的姿態是在報告系統狀態。
    pub tail: (u8, u8, u8, u8),
    /// 尾尖最多抬高幾個子像素。
    pub tail_lift: u8,
    /// 抬起來的形狀 —— 見 [`Curve`]。
    pub tail_curve: Curve,
    /// 動畫循環。至少一格。
    pub frames: &'static [Frame],
}

/// 露出來的那一塊剪影。
#[derive(Debug, Clone)]
pub struct Reveal {
    pub lines: Vec<String>,
    pub w: u16,
    pub h: u16,
}

impl Reveal {
    fn empty() -> Self {
        Self {
            lines: Vec::new(),
            w: 0,
            h: 0,
        }
    }
}

/// 從頭頂往下露出 `rows` 列，並把左右的空白裁掉。
///
/// # 為什麼是「露出多少」而不是「固定裁半隻」
///
/// 完整剪影要一整塊 44×16 的乾淨區域。密集的頁面（總覽把面板鋪滿整頁）
/// 最大的乾淨區塊常常只有幾列 —— 所以最該有品牌感的地方反而看不到。
///
/// 露出的列數與寬度都跟著走：只露頭的時候寬度也只有十幾欄，所以窄的
/// 終端機一樣放得下。這是「牠從儀表板後面探出多少」，不是分級。
///
/// 裁切框一律用**第 0 格**算。各格自己算的話，眨眼那格少一點墨、
/// 尾巴那格多一欄，露出的範圍就會每幀跳動。
pub fn reveal(species: Species, state: State, frame: u64, rows: u16) -> Reveal {
    let pose = pose(species, state);
    let (pw, ph, base) = rasterize(pose, 0);
    let raw = to_lines(pw, ph, &base);
    let rows = rows.min(raw.len() as u16) as usize;
    if rows == 0 {
        return Reveal::empty();
    }
    let (mut x0, mut x1) = (usize::MAX, 0usize);
    for l in raw.iter().take(rows) {
        for (x, c) in l.chars().enumerate() {
            if c != ' ' {
                x0 = x0.min(x);
                x1 = x1.max(x);
            }
        }
    }
    if x0 > x1 {
        return Reveal::empty();
    }
    // 會翹尾巴的姿態，尾巴勾起來時左右各多一欄
    if pose.frames.iter().any(|f| f.tail != 0) {
        let canvas = raw.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        x0 = x0.saturating_sub(1);
        x1 = (x1 + 1).min(canvas.saturating_sub(1));
    }

    let (fw, fh, bits) = rasterize(pose, frame as usize);
    let lines: Vec<String> = to_lines(fw, fh, &bits)
        .iter()
        .take(rows)
        .map(|l| {
            let row: Vec<char> = l.chars().collect();
            (x0..=x1)
                .map(|x| row.get(x).copied().unwrap_or(' '))
                .collect()
        })
        .collect();
    Reveal {
        w: (x1 - x0 + 1) as u16,
        h: lines.len() as u16,
        lines,
    }
}

/// 算出這個姿態在某一格動畫時的點陣。
///
/// 回傳 `(width, height, bits)`，`bits` 是 row-major 的布林。
pub fn rasterize(pose: &Pose, frame: usize) -> (usize, usize, Vec<bool>) {
    rasterize_art(
        pose.art,
        pose.frames,
        frame,
        pose.back,
        pose.ears,
        pose.tail,
        (pose.tail_lift, pose.tail_curve),
    )
}

fn rasterize_art(
    art: &'static [&'static str],
    frames: &[Frame],
    frame: usize,
    back: (u8, u8),
    ears: (u8, u8),
    tail: (u8, u8, u8, u8),
    tail_motion: (u8, Curve),
) -> (usize, usize, Vec<bool>) {
    let h = art.len();
    let w = art.iter().map(|r| r.chars().count()).max().unwrap_or(0);
    let f = frames.get(frame % frames.len().max(1)).unwrap_or(&STILL);
    let mut bits = vec![false; w * h];

    for (y, row) in art.iter().enumerate() {
        for (x, c) in row.chars().enumerate() {
            if c == '#' {
                bits[y * w + x] = true;
            }
        }
    }
    if f.breath {
        morph_top(&mut bits, w, h, back);
    }
    if f.ears {
        morph_top(&mut bits, w, h, ears);
    }
    if f.tail != 0 {
        // 1 = 甩到一半、2 = 完全勾起
        let (lift, curve) = tail_motion;
        let lift = (lift as u32 * f.tail as u32 / 2) as u8;
        curl_tail(&mut bits, w, h, tail, lift, curve);
    }
    for &(x, y) in f.clear {
        let (x, y) = (x as usize, y as usize);
        if x < w && y < h {
            bits[y * w + x] = false;
        }
    }
    for &(x, y) in f.set {
        let (x, y) = (x as usize, y as usize);
        if x < w && y < h {
            bits[y * w + x] = true;
        }
    }
    (w, h, bits)
}

/// 把一段範圍內的上緣輪廓**往上移動**一個子像素。
///
/// 清掉原本的那一格、點亮上面那一格 —— 淨厚度不變，輪廓真的移動了。
/// 只加不清的話是把輪廓變粗，看起來像多描了一層邊。
fn morph_top(bits: &mut [bool], w: usize, h: usize, span: (u8, u8)) {
    let (x0, x1) = (span.0 as usize, span.1 as usize);
    if x1 <= x0 || x0 >= w {
        return;
    }
    for x in x0..=x1.min(w - 1) {
        let Some(top) = (0..h).find(|&y| bits[y * w + x]) else {
            continue;
        };
        if top == 0 {
            continue;
        }
        bits[top * w + x] = false;
        bits[(top - 1) * w + x] = true;
    }
}

/// 三個數的中位數：把 `v` 夾在左右鄰居之間。
fn median3(l: usize, v: usize, r: usize) -> usize {
    l.min(r).max(v.min(l.max(r)))
}

/// 把尾巴從「垂下來」變成「往上勾」。
///
/// 三段：
/// 1. **每一欄算出搬完後的上下緣**。上緣照 [`Curve::at`] 抬、粗細照
///    [`Curve::thin`] 收、根部下緣照 [`Curve::fillet`] 黏著身體；跟前一欄
///    碰不到就補到接上（尾尖只剩一兩點時特別容易斷成一串點）；下緣往尾尖
///    走不能掉超過 [`Curve::sag`]。沒抬的欄也記下它原本的上下緣 —— 抬升
///    起點那一欄才有鄰居可以對。
/// 2. **邊緣平滑**：上下緣各做一次寬度 3 的中位數，一欄不能比左右鄰居都
///    凸或都凹。抬升量跟粗細各自四捨五入，交界處會留下一欄寬的缺口或小刺。
/// 3. **畫**：框裡每一欄清掉再照上下緣填實。
///
/// 只動 `span` 這個框裡的點；身體、頭、腳都在框外，所以坐姿不變、腳底不動。
fn curl_tail(
    bits: &mut [bool],
    w: usize,
    h: usize,
    span: (u8, u8, u8, u8),
    lift: u8,
    curve: Curve,
) {
    let (x0, x1, y0, y1) = (
        span.0 as usize,
        span.1 as usize,
        span.2 as usize,
        span.3 as usize,
    );
    if x1 <= x0 || y1 < y0 || x0 >= w || y0 >= h || lift == 0 {
        return;
    }
    let (x1, y1) = (x1.min(w - 1), y1.min(h - 1));
    let span_w = (x1 - x0) as f32;

    // 一、每一欄的上下緣
    let mut runs: Vec<Option<(usize, usize)>> = vec![None; x1 - x0 + 1];
    let mut prev: Option<(usize, usize)> = None;
    for x in (x0..=x1).rev() {
        // t = 0 在根部，t = 1 在尾尖
        let t = (x1 - x) as f32 / span_w;
        let (mut top, mut bottom, mut n) = (usize::MAX, 0usize, 0usize);
        for y in y0..=y1 {
            if bits[y * w + x] {
                top = top.min(y);
                bottom = bottom.max(y);
                n += 1;
            }
        }
        if n == 0 {
            continue;
        }
        let up = ((lift as f32 * curve.at(t)).round() as usize).max(curve.floor());
        if up == 0 {
            runs[x - x0] = Some((top, bottom));
            prev = Some((top, bottom));
            continue;
        }
        let s = t * t * (3.0 - 2.0 * t);
        let thickness = ((n as f32) * (1.0 - curve.thin() * s)).round().max(1.0) as usize;
        let lag = if curve.fillet() > 0.0 {
            (up as f32 * (1.0 - (t / curve.fillet()).min(1.0))).round() as usize
        } else {
            0
        };
        let mut top = top.saturating_sub(up);
        let mut bottom = (top + thickness - 1 + lag).min(h - 1);
        if let Some((ptop, pbottom)) = prev {
            if top > pbottom + 1 {
                top = pbottom + 1;
            } else if bottom + 1 < ptop {
                bottom = ptop - 1;
            }
            if bottom > pbottom + curve.sag() {
                bottom = (pbottom + curve.sag()).max(top);
            }
        }
        runs[x - x0] = Some((top, bottom));
        prev = Some((top, bottom));
    }

    // 二、邊緣平滑（由左往右、就地更新 —— 跟離線模擬器一模一樣）
    for i in 1..runs.len().saturating_sub(1) {
        let (Some(l), Some(c), Some(r)) = (runs[i - 1], runs[i], runs[i + 1]) else {
            continue;
        };
        let top = median3(l.0, c.0, r.0);
        let bottom = median3(l.1, c.1, r.1);
        runs[i] = Some((top.min(bottom), bottom));
    }

    // 三、畫
    for (i, run) in runs.iter().enumerate() {
        let Some((top, bottom)) = *run else { continue };
        let x = x0 + i;
        for y in y0..=y1 {
            bits[y * w + x] = false;
        }
        for y in top..=bottom.min(h - 1) {
            bits[y * w + x] = true;
        }
    }
}

/// 四分格：一個字元格表達 2×2 個子像素。
///
/// 用四分格而不是上下半格，橫向解析度直接加倍 —— 同樣的畫面面積，
/// 腿的關節、鹿角的分叉、頭部的線條都留得住。
/// 兩者同屬 U+2580 區段（CP437 時代就有），字型支援沒有差別。
const QUAD: [char; 16] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
];

/// 把子像素點陣壓成四分格字元列。2×2 個子像素變成一個字元。
pub fn to_lines(w: usize, h: usize, bits: &[bool]) -> Vec<String> {
    let at = |x: usize, y: usize| x < w && y < h && bits[y * w + x];
    let mut out = Vec::with_capacity(h.div_ceil(2));
    let mut y = 0;
    while y < h {
        let mut line = String::with_capacity(w / 2);
        let mut x = 0;
        while x < w {
            let v = (at(x, y) as usize)
                | (at(x + 1, y) as usize) << 1
                | (at(x, y + 1) as usize) << 2
                | (at(x + 1, y + 1) as usize) << 3;
            line.push(QUAD[v]);
            x += 2;
        }
        // 右邊的空白沒有意義，去掉可以少寫一些格子
        out.push(line.trim_end().to_owned());
        y += 2;
    }
    out
}

/// 找出某個種類 / 姿態的定義。
pub fn pose(species: Species, state: State) -> &'static Pose {
    let table = match species {
        Species::Fox => FOX,
        Species::Deer => DEER,
    };
    table
        .iter()
        .find(|p| p.state == state)
        // 每個種類都必須有 observe，有測試在守
        .unwrap_or(&table[0])
}

/// 地面裝飾：短橫與草叢，寬度自適應。
///
/// concept 圖裡每隻動物腳下都有這條線，它讓剪影「站在某個地方」，
/// 而不是浮在空中。
pub fn ground(width: usize, seed: u64) -> String {
    const TUFTS: [char; 6] = ['▁', '▂', '▁', '▃', '▁', '▂'];
    let mut s = String::with_capacity(width);
    // 跟 motif::circuit 同一個理由：`| 1` 會讓相鄰的 seed 撞在一起
    let mut r = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x5DEE_CE66_D000_0001);
    for i in 0..width {
        // 便宜的確定性亂數：同一個 seed 永遠給同一條地面，
        // 畫面才不會每格重畫都在跳。
        r = r
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let pick = (r >> 33) % 10;
        s.push(if pick < 5 {
            ' '
        } else if pick < 8 {
            '▁'
        } else {
            TUFTS[i % TUFTS.len()]
        });
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────
// 點陣
//
// 都是面向右邊的側面剪影，112 × 32 個子像素 —— 四分格壓縮後
// 佔 56 欄 × 16 列。
//
// 輪廓是**從 concept 圖直接萃取**的：切掉卡片外框與說明文字，
// 裁到動物的外接框，再用面積平均降採樣到 56 欄。
//
// 試過改用多邊形重新描邊，線條是比較滑順，但也把原圖的個性磨掉了 ——
// 腿的關節、身體上的分隔線、頭部的細節全都變成一團光滑的剪影。
// 那些「破洞」不是雜訊，是原作者畫的線。
//
// 只有這一個尺寸。降到 28 欄時細腿會碎成一格一格，
// 與其塞一隻醜的，不如不畫。
// 試過直接把 concept 圖降取樣，但這個尺寸下細腿會被吃掉、內部的分隔線
// 會變成破洞，怎麼調參數都是一團糊 —— 描邊才是對的做法。
//
// 尺寸也是試出來的：再小一號，狐狸的四條腿和鹿角的分叉就糊在一起；
// 再大一號就開始搶監控面板的空間。
// ─────────────────────────────────────────────────────────────────────────

#[rustfmt::skip]
static FOX: &[Pose] = &[
    Pose {
        state: State::Observe,
        art: &[
            "                                                                                    #     ##                    ",
            "                                                                                    ###  #  #                   ",
            "                                                                                    ######  ##                  ",
            "                                                                                    ######   ###                ",
            "                                                                                  ##############                ",
            "                                                                                #################               ",
            "                                                                              ###   #############               ",
            "                                                                          ########################              ",
            "                                                                           #######     ###########              ",
            "                                                                                   ##       #######             ",
            "                                                                                 ######       #####             ",
            "                                                                                ########      ####              ",
            "                                                                          ###############      ####             ",
            "                                                          #############################         ###             ",
            "                                                     ###################################        ###             ",
            "                                             ############################################      ###              ",
            "                                          ############################################ ###     ###              ",
            "                                       ########## #####################################        ##               ",
            "                                  #############  ############# ########################       ###               ",
            "                            ##################   #############     ########### ########      #                  ",
            "                       #####################     ############                  ######      #                    ",
            "                     #####################       ###########       #           ######     #                     ",
            "                   ######################       ##########         #           ######    ##                     ",
            "                 #####################         #########          #             #####   ###                     ",
            "                 #################          ###########        ##               #####   ##                      ",
            "                #############             #########      ######                  ####   ##                      ",
            "                ########                 #####          ####                     ####  ##                       ",
            "            #   ###                      ####         ####                        ###  ##                       ",
            "            #   #                       ###           ####                        ###  ##                       ",
            "            #                          ###            ####                         ######                       ",
            "              # #                     ####              ###                         ####         #              ",
            "            # # ##                    ######            ######                       ######    #                ",
        ],
        back: (28, 72),
        ears: (82, 98),
        tail: (14, 36, 17, 27),
        tail_lift: 18,
        tail_curve: Curve::Wave,
        frames: &[
            STILL,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(90, 8), (91, 8), (90, 9), (91, 9)], set: &[] },
            STILL,
            STILL,
            STILL,
            STILL,
            BREATH,
            BREATH,
            STILL,
            STILL,
            STILL,
            EARS,
            STILL,
            STILL,
            STILL,
            // 尾巴翹起來，保持一下，再放下。
            // 中間那格 STILL 是刻意的：抬起 → 停 → 放下 → 再抬一次，
            // 比一路抬著自然，也不會變成等距的節拍。
            TAIL_MID,
            TAIL_UP,
            TAIL_UP,
            TAIL_UP,
            TAIL_UP,
            TAIL_MID,
            STILL,
            TAIL_MID,
            TAIL_UP,
            TAIL_MID,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(90, 8), (91, 8), (90, 9), (91, 9)], set: &[] },
            STILL,
            STILL,
            BREATH,
            STILL,
        ],
    },
    Pose {
        state: State::Explore,
        art: &[
            "                                                                                                                ",
            "                                                                                                ##   ##         ",
            "                                                                                               #### ###         ",
            "                                                                                             ###  # ###         ",
            "                                                                                          #####   #####         ",
            "                                                                                        #######   #####         ",
            "                                                                                       ####### #########        ",
            "                                                                                   #######################      ",
            "                                                                              ############################      ",
            "                                                  ###################### ############################   ###     ",
            "                                            ##################################################################  ",
            "                                 ################################################################# #############",
            "                            ####################################################################         ###### ",
            "                          ############# ###################################################                     ",
            "                        #############   #####################################################                   ",
            "                     #############      ##################################### #############                     ",
            "                    #############       #################  ################## #############                     ",
            "             ####################       ################      ###############  ############                     ",
            "         ######################        ##############               ########    #########                       ",
            "       #####################           ##############        #                  #########                       ",
            "     ######################           ##############         #                  ##########                      ",
            "    ################# ##             ############          #             #####    ##########                    ",
            "   #################              ############            ##           ######        #########                  ",
            " ################              ############       ##    ###            #####            ########                ",
            " ############                 ######              ##  ###              ####                 ######              ",
            "##################           #####               ######               ####                   ########           ",
            "########                     ####                #####               #####                      #####           ",
            "#####                      #####                 #####               ####                        ######         ",
            "#                         #####                   ######           #####                           ######       ",
            "                          ####                      #####          ####                              ######     ",
            "                        ######                       ######        ####                                #####    ",
            "                 #      ######                        ########     #######                 #                    ",
        ],
        back: (24, 68),
        ears: (94, 106),
        tail: (0, 0, 0, 0),
        tail_lift: 0,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            EARS,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(98, 6), (99, 6), (98, 7), (99, 7)], set: &[] },
            STILL,
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            EARS,
            STILL,
        ],
    },
    Pose {
        state: State::Proceed,
        art: &[
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                             ###########                ##########                                              ",
            "                        #####################    #####################                                          ",
            "                     ########################################################                    ##    #        ",
            "                 ############################# ###################################              ## #  ##        ",
            "            ###########################      ##########################################       ###  # ###        ",
            "           ######################          ######################################################  ## ##        ",
            "        #####################            #######################################################   #####        ",
            "      ####################               #################     ################################# ########       ",
            "   #####################                ################        ##########################################      ",
            " ##################                   ################                ##### ################################    ",
            " #################                   ###############                   ###  ################################    ",
            "#############                      #############           #              ########## ###### ##########   ###    ",
            "   #################            ############             #             ###########    ######  ################  ",
            "       ######                 ########           #     ##             ##########                       #########",
            "                            ######              #######               # #############                     ##### ",
            "                            #####              ######                 #     ############                        ",
            "                           ####                ####                 ###       ############                      ",
            "                          ####                 ######              ####           #########                     ",
            "                        ######                   #####             ###                #######                   ",
            "                        ####                      ####             ###                    #####                 ",
            "                        #####                      ########        ####                    ##########           ",
            "                        #######                       ######       ######                    #########          ",
        ],
        back: (16, 44),
        ears: (86, 104),
        tail: (0, 0, 0, 0),
        tail_lift: 0,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(100, 19), (101, 19), (100, 20), (101, 20)], set: &[] },
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            STILL,
        ],
    },
    Pose {
        state: State::Rest,
        art: &[
            "                                                                               #   ##                           ",
            "                                                                             ## #####                           ",
            "                                                                            ### #####                           ",
            "                                                                          ####  ######                          ",
            "                                                                         ####   ########                        ",
            "                                                                        #################                       ",
            "                                                                      ###############  ###                      ",
            "                                                                       #######################                  ",
            "                                                                      #############      #####                  ",
            "                                                                       ########                                 ",
            "                                                                     #########      #                           ",
            "                                                                     #########     ##                           ",
            "                                                                     #########     ###                          ",
            "                                                                   ############     ##                          ",
            "                                                                 ##############     ##                          ",
            "                                                              ##################    ##                          ",
            "                                                            ####################   ##                           ",
            "                                                            ####################   ##                           ",
            "                                                          ############## ######    #                            ",
            "                                                          #############  ######   #                             ",
            "                                                        #############    ######  ##                             ",
            "                                                        ############      ##### ##                              ",
            "                                                       #############      ##### ##                              ",
            "                                ################       #################   #### ##                              ",
            "                             ####################      ##################  #### ## ##                           ",
            "                          ##########################   ###################  ### ## ##                           ",
            "                         ###########################   ###################  ### #  ##                           ",
            "                        ###################     ###### ##################   ####   ##                           ",
            "                       #############              #### #################     ###   ##                           ",
            "                       #############                 ## ############          ###  ##                           ",
            "                       #######                          ##########   ######   #### ####                         ",
            "                 #     ####                 ######   ########################  ##### ####  #                    ",
        ],
        back: (52, 68),
        ears: (74, 90),
        // 右界停在 43：44、45 兩欄的最底列是臀部的底邊，不是尾巴 ——
        // 框進來會跟著抬，臀部底下就缺一角。
        tail: (23, 43, 22, 31),
        tail_lift: 12,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            STILL,
            STILL,
            BREATH,
            BREATH,
            BREATH,
            STILL,
            STILL,
            // 趴著的時候尾巴翹得比較久、比較慢 —— 這隻正在休息
            TAIL_MID,
            TAIL_UP,
            TAIL_UP,
            TAIL_UP,
            TAIL_UP,
            TAIL_UP,
            TAIL_MID,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(82, 6), (83, 6), (82, 7), (83, 7)], set: &[] },
            STILL,
            STILL,
            STILL,
            BREATH,
            BREATH,
            STILL,
            STILL,
        ],
    },
    Pose {
        state: State::Return,
        art: &[
            "                                                                               #     ###                        ",
            "                                                                               ####  ## #                       ",
            "                                                                               ##### ## ###                     ",
            "                                                                                 ######  ###                    ",
            "                                                                                 #######  ###                   ",
            "                                                                               ################                 ",
            "                                                                              ##################                ",
            "                                                                             ###  ##############                ",
            "                                                                          ########################              ",
            "                                                                          ######      ############              ",
            "                                                                                 ##       ########              ",
            "                                                #######################      ########       #######             ",
            "                                           ##########################################       #######             ",
            "                                        ##############################################        #####             ",
            "                                    ######### #########################################       ####              ",
            "                                 ##########  ######################################## #      #####              ",
            "                              ###########    ############### ########################        ###                ",
            "                             ############    ###############  #######################        ###                ",
            "                          #############       ##############   ######################      ####                 ",
            "                      #################       #############         ######  ########       ##                   ",
            "                   ##################          ###########                  #######      ##                     ",
            "                 ###################          ###########                   #######     ##                      ",
            "                ##################           ##########        #           #######     ###                      ",
            "               #################            #########        ##              #####    ###                       ",
            "              ####################        ########        ###                ######  ###                        ",
            "              #############              #####          ##                    #####  ###                        ",
            "             #############              ####         ###                       ####  ##                         ",
            "             #########                  ###         ####                        ###  ##                         ",
            "             #######                  ####          ####                        #### ##                         ",
            "             #####                    ###            ###                         ### ##                         ",
            "              ####                   ####            ####                        ######                         ",
            "               ##                    #####            ######                      ########                      ",
        ],
        back: (28, 72),
        ears: (78, 94),
        tail: (0, 0, 0, 0),
        tail_lift: 0,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(86, 8), (87, 8), (86, 9), (87, 9)], set: &[] },
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            STILL,
            EARS,
            STILL,
            STILL,
            STILL,
            STILL,
        ],
    },
];

#[rustfmt::skip]
static DEER: &[Pose] = &[
    Pose {
        state: State::Observe,
        art: &[
            "                                                                 ## #                #                          ",
            "                                                                 #### ##        #   ##                          ",
            "                                                                  ### #          #  #   #                       ",
            "                                                                   ####   #   #  ###  ###                       ",
            "                                                                     #######  #  #######                        ",
            "                                                                     ##  ####  #####                            ",
            "                                                                     ####  ######                               ",
            "                                                                      ######## #####                            ",
            "                                                                          ###########                           ",
            "                                                                          #######                               ",
            "                                                                          ######                                ",
            "                                                                         #######                                ",
            "                                                                         ########                               ",
            "                                                                     ############                               ",
            "                                                      ###########################                               ",
            "                                               ##################################                               ",
            "                                            #####################################                               ",
            "                                          #######################################                               ",
            "                                         ########################################                               ",
            "                                         ############### #######################                                ",
            "                                         #  ###########   ########## ######  ##                                 ",
            "                                            ##########               #####  ##                                  ",
            "                                           #########   ####          ####  ##                                   ",
            "                                          #######     ###            ####  #                                    ",
            "                                        #######     ####             #### ##                                    ",
            "                                      #####        ###                ##  ##                                    ",
            "                                      ###        ####                 ##  ##                                    ",
            "                                     ###          ##                  ##  #                                     ",
            "                      #              ##            ###                 #  #              #                      ",
            "                      #             ##              ###                ## #              #                      ",
            "                    #   #          ###               ###               ## #             #  #                    ",
            "                     ## # #        ##                 ###           #  #####         ## ##                      ",
        ],
        back: (40, 64),
        ears: (0, 0),
        tail: (0, 0, 0, 0),
        tail_lift: 0,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(78, 8), (79, 8), (78, 9), (79, 9)], set: &[] },
            STILL,
            STILL,
            STILL,
            STILL,
            BREATH,
            BREATH,
            STILL,
            STILL,
            STILL,
            EARS,
            STILL,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(78, 8), (79, 8), (78, 9), (79, 9)], set: &[] },
            STILL,
            STILL,
            BREATH,
            STILL,
        ],
    },
    Pose {
        state: State::Explore,
        art: &[
            "                                                                              #      #                          ",
            "                                                                            ##      ##                          ",
            "                                                                            ## #   ##                           ",
            "                                                                             # #  ##     #                      ",
            "                                                                             #### ##    ##   #                  ",
            "                                                                               #####   ##   ##                  ",
            "                                                                                 #######    #                   ",
            "                                                                               ###   #######                    ",
            "                                                                                ####  ###                       ",
            "                                                                                 ###########                    ",
            "                                                                                   ##########                   ",
            "                                                                                ################                ",
            "                                                                 #######     ####################               ",
            "                                      #################################################                         ",
            "                                    #################################################                           ",
            "                                 ###################################################                            ",
            "                               ### ################################################                             ",
            "                              ###  #############################################                                ",
            "                              #     ###########################################                                 ",
            "                                    ############# ################ ########                                     ",
            "                                    ############       #########   ##########                                   ",
            "                                   ###########                       #########                                  ",
            "                               ############    ##                ####   #######                                 ",
            "                             #########       ###                 ###        #####                               ",
            "                             #####         ####                 ####          ####                              ",
            "                             ##           ####                  ###             ###                             ",
            "                            ###           ####                 ###               ###                            ",
            "                           ###             ####                ##                 ###                           ",
            "                 #        ###               ####              ###                  ###                          ",
            "                 #       ###                  ###            ###                    ###                         ",
            "               ## # #   ####                   ####         ####           #         ###                        ",
            "                # # #   ####     ##             ####        ####       #  #           ###    # ##               ",
        ],
        back: (32, 70),
        ears: (0, 0),
        tail: (0, 0, 0, 0),
        tail_lift: 0,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            EARS,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(88, 8), (89, 8), (88, 9), (89, 9)], set: &[] },
            STILL,
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            EARS,
            STILL,
        ],
    },
    Pose {
        state: State::Proceed,
        art: &[
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                                                ",
            "                                                                                        ## ##     #             ",
            "                                                                                        ## ##   ###             ",
            "                                            ####                                         ####   ###             ",
            "                                   #################                                      ### ###     #     ##  ",
            "                             #######################################                       ######    ##    ###  ",
            "                        #################################################                    ###    ##    ###   ",
            "                    #########################################################                 ###  ##    ###    ",
            "                  #############################################################          ###    #####   ###    #",
            "                ###################################################################       #####  ####   #    ## ",
            "               ########################### ###########################################     #####   ## ### ####  ",
            "              ###### #################### ################################################# #####  #########    ",
            "             #####     #################  #############################################################         ",
            "              #       ################       ##############  ############ #############################         ",
            "                      ##############                         ########## ################################        ",
            "                     #############      ####              ###  ##########     ###################### ####       ",
            "                   ############      ######               #####   #######                    #############      ",
            "                ###########        ######                 #####     ######                      ###########     ",
            "             #########            #####                   ####        ######                        ########    ",
            "            ########            #####                    ####          ######                         ####      ",
            "           ######             ######                     ####            #####                                  ",
            "           ####               #####                      ###                ###                                 ",
            "           ####                 ####                    ###                 ####                                ",
            "           ###                   ####                   ###                  ####                               ",
            "         ####                     ####                 ###                    ####                              ",
            "         ####                       ####              ###                       ###                             ",
            "  #     ####                         ####            ###                        ####                            ",
            "#   #   ####      ##        #         ####           ###                         ####              #            ",
            "## ##  ####       # #      ##          #####        ####                 ##       ####        #  ##        #    ",
        ],
        back: (20, 68),
        ears: (0, 0),
        tail: (0, 0, 0, 0),
        tail_lift: 0,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(96, 6), (97, 6), (96, 7), (97, 7)], set: &[] },
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            STILL,
        ],
    },
    Pose {
        state: State::Rest,
        art: &[
            "                                                                          #      #                              ",
            "                                                                        ###     ### #                           ",
            "                                                                    #   ###      ## #                           ",
            "                                                                   ##   ##  #     ###                           ",
            "                                                                   ###  ##  #    ###                            ",
            "                                                                    #########    ##    #                        ",
            "                                                                       ########  #   ###                        ",
            "                                                                            #######  #                          ",
            "                                                                         ###   #######   #                      ",
            "                                                                         #####    ###  ###                      ",
            "                                                                          ######  #######                       ",
            "                                                                           ############                         ",
            "                                                                               ##########                       ",
            "                                                                             ########  ####                     ",
            "                                                                             ################                   ",
            "                                                                            ###################                 ",
            "                                                                           ###########   #####                  ",
            "                                                                           ###########                          ",
            "                                                                          ############                          ",
            "                                          ########                        #############                         ",
            "                                    #########################    ######### ############                         ",
            "                                #######################################################                         ",
            "                            ############################################################                        ",
            "                           ###################################################### ######                        ",
            "                          ######################### ############################## #####                        ",
            "                        ############################ #############################  ####                        ",
            "                        ############################ #############################  ###                         ",
            "               #       ###########################  ### ########### ##############   #                          ",
            "               ##      ##### ###################         ########  ############     ###        #                ",
            "                #      ###### #######  #######       ####          ################# ####      #                ",
            "              ##  #     ######       #####   ###############  ##    ######################   #   #              ",
            "          # # ## ## #     ####     ############################# # # ###################     # # #  #           ",
        ],
        back: (28, 64),
        ears: (0, 0),
        tail: (0, 0, 0, 0),
        tail_lift: 0,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            STILL,
            STILL,
            BREATH,
            BREATH,
            BREATH,
            STILL,
            STILL,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(80, 10), (81, 10), (80, 11), (81, 11)], set: &[] },
            STILL,
            STILL,
            STILL,
            BREATH,
            BREATH,
            STILL,
            STILL,
        ],
    },
    Pose {
        state: State::Return,
        art: &[
            "                                                                        ##   ##                                 ",
            "                                                                    #     #####  #                              ",
            "                                                                    #   ##    ## #                              ",
            "                                                                    ##    ##  ####                              ",
            "                                                                      ##  ## ###                                ",
            "                                                                   ###########                                  ",
            "                                                                       ##########                               ",
            "                                                                       ###########                              ",
            "                                                                    ###############                             ",
            "                                                                    #####     ######                            ",
            "                                                               ######         #######                           ",
            "                                                          #################    #######                          ",
            "                                                       ###############################                          ",
            "                                                   ####################################                         ",
            "                                                 ######################################                         ",
            "                                                #####  ###############################                          ",
            "                                               #####  # ############ #################                          ",
            "                                               ###   #  ###########  ################                           ",
            "                                               #    ###  ##########    ###########                              ",
            "                                                   ####   ########       #   ####                               ",
            "                                                  ######  ######         ##  ####                               ",
            "                                                ######   ######           ## ###                                ",
            "                                              ######     #####            ### ##                                ",
            "                                            #####      #####               ## ##                                ",
            "                                           ####        ####                ## ##                                ",
            "                                           ###          ###                ## ##                                ",
            "                                          ###            ###                # ###                               ",
            "                      #                  ###              ###               ## ##                               ",
            "                       #    #            ##                ###              ## ###         #                    ",
            "                    #  #  ##            ##                  ###             ## ###        ##                    ",
            "                      ##  #  #         ###           #       ###    ##       ## ###     # #   #                 ",
            "                 # #  ## # # ##       ##### ### #  ## ##     ####  # ## # ## ####### #  #######                 ",
        ],
        back: (44, 64),
        ears: (0, 0),
        tail: (0, 0, 0, 0),
        tail_lift: 0,
        tail_curve: Curve::Arc,
        frames: &[
            STILL,
            STILL,
            STILL,
            STILL,
            Frame { breath: false, ears: false, tail: 0, clear: &[(74, 6), (75, 6), (74, 7), (75, 7)], set: &[] },
            STILL,
            STILL,
            BREATH,
            STILL,
            STILL,
            STILL,
            EARS,
            STILL,
            STILL,
            STILL,
            STILL,
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn all_poses() -> impl Iterator<Item = (Species, &'static Pose)> {
        FOX.iter()
            .map(|p| (Species::Fox, p))
            .chain(DEER.iter().map(|p| (Species::Deer, p)))
    }

    #[test]
    fn every_species_has_every_state() {
        for sp in [Species::Fox, Species::Deer] {
            for st in State::ALL {
                let p = pose(sp, *st);
                assert_eq!(p.state, *st, "{} 缺少 {} 姿態", sp.name(), st.name());
            }
        }
    }

    #[test]
    fn state_names_match_the_config_allowlist() {
        // config 驗證用的是 STATE_NAMES，兩邊不同步的話設定會被誤判成無效
        assert_eq!(STATE_NAMES.len(), State::ALL.len());
        for n in STATE_NAMES {
            assert!(State::parse(n).is_some(), "{n} 不是有效狀態");
        }
    }

    #[test]
    fn art_rows_are_all_the_same_width_and_only_use_hash_or_space() {
        for (sp, p) in all_poses() {
            for art in [p.art] {
                let w = art[0].chars().count();
                for (i, row) in art.iter().enumerate() {
                    assert_eq!(
                        row.chars().count(),
                        w,
                        "{} {} 第 {i} 列寬度不一致",
                        sp.name(),
                        p.state.name()
                    );
                    assert!(
                        row.chars().all(|c| c == '#' || c == ' '),
                        "{} {} 第 {i} 列有非法字元",
                        sp.name(),
                        p.state.name()
                    );
                }
            }
        }
    }

    #[allow(dead_code)]
    fn old_art_check() {
        for (sp, p) in all_poses() {
            let w = p.art[0].chars().count();
            for (i, row) in p.art.iter().enumerate() {
                assert_eq!(
                    row.chars().count(),
                    w,
                    "{} {} 第 {i} 列寬度不一致",
                    sp.name(),
                    p.state.name()
                );
                assert!(
                    row.chars().all(|c| c == '#' || c == ' '),
                    "{} {} 第 {i} 列有非法字元",
                    sp.name(),
                    p.state.name()
                );
            }
        }
    }

    #[test]
    fn every_blink_actually_lands_on_the_face() {
        // 眨眼是「把眼睛那幾格關掉」。座標標錯就是關掉一片本來就空的地方 ——
        // 動畫照跑、畫面沒變，而且完全看不出哪裡錯了（FOX PROCEED 就是這樣）。
        for (sp, p) in all_poses() {
            let (w, h, base) = rasterize(p, 0);
            let mut total = 0;
            let mut hits = 0;
            for f in p.frames {
                for &(x, y) in f.clear {
                    total += 1;
                    if (x as usize) < w && (y as usize) < h && base[y as usize * w + x as usize] {
                        hits += 1;
                    }
                }
            }
            assert!(total > 0, "{} {} 一格眨眼都沒有", sp.name(), p.state.name());
            assert!(
                hits * 2 >= total,
                "{} {} 的眨眼座標有一半以上落在空白處（{hits}/{total}），等於沒眨",
                sp.name(),
                p.state.name()
            );
        }
    }

    #[test]
    fn breathing_moves_the_contour_instead_of_thickening_it() {
        // 早期版本只「在上緣多點亮一格」而不清掉舊的，那是把輪廓加粗，
        // 畫面上看起來像剪影外面多描了一層邊。真正的呼吸應該是輪廓移動：
        // 清一格、點一格，淨墨量不變。
        for (sp, p) in all_poses() {
            let (_, _, base) = rasterize(p, 0);
            let ink0 = base.iter().filter(|b| **b).count() as i64;
            for (i, f) in p.frames.iter().enumerate() {
                if !f.breath || !f.clear.is_empty() {
                    continue; // 只看純呼吸的那幾格
                }
                let (_, _, bits) = rasterize(p, i);
                let ink = bits.iter().filter(|b| **b).count() as i64;
                assert_eq!(
                    ink,
                    ink0,
                    "{} {} 第 {i} 格呼吸改變了墨量（{ink0} → {ink}）—— 那是加厚不是移動",
                    sp.name(),
                    p.state.name()
                );
            }
        }
    }

    #[test]
    fn animation_never_moves_the_baseline_or_the_silhouette_bounds() {
        // 腳底會漂、整隻左右晃，都會讓人覺得畫面在抖。
        //
        // 唯一的例外是搖尾巴：尾巴甩出去時左緣本來就會多一個子像素，
        // 那正是這個動作。但也只有一個子像素，而且只在有尾巴動作的格。
        for (sp, p) in all_poses() {
            let (w, h, base) = rasterize(p, 0);
            let bbox = |bits: &[bool]| {
                let (mut t, mut b, mut l, mut r) = (h, 0usize, w, 0usize);
                for y in 0..h {
                    for x in 0..w {
                        if bits[y * w + x] {
                            t = t.min(y);
                            b = b.max(y);
                            l = l.min(x);
                            r = r.max(x);
                        }
                    }
                }
                (t, b, l, r)
            };
            let (_, b0, l0, r0) = bbox(&base);
            for (i, f) in p.frames.iter().enumerate() {
                let (_, _, bits) = rasterize(p, i);
                let (_, b, l, r) = bbox(&bits);
                assert_eq!(b, b0, "{} {} 第 {i} 格腳底漂了", sp.name(), p.state.name());
                let slack = if f.tail != 0 { 1 } else { 0 };
                assert!(
                    l0.abs_diff(l) <= slack && r0.abs_diff(r) <= slack,
                    "{} {} 第 {i} 格整隻左右移動了（{l0}..{r0} → {l}..{r}）",
                    sp.name(),
                    p.state.name()
                );
            }
        }
    }

    #[test]
    fn the_tail_curl_only_touches_the_tail() {
        // 「小動作」跟「換姿勢」的差別就在這裡：身體、腳、頭一格都不能動。
        for (sp, p) in all_poses() {
            let (w, _, base) = rasterize(p, 0);
            let (tx0, tx1, ty0, ty1) = p.tail;
            for (i, f) in p.frames.iter().enumerate() {
                if f.tail == 0 {
                    continue;
                }
                assert!(
                    tx1 > tx0 && p.tail_lift > 0,
                    "{} {} 有翹尾巴的格，卻沒有定義尾巴範圍或幅度",
                    sp.name(),
                    p.state.name()
                );
                let (_, _, bits) = rasterize(p, i);
                let mut changed = 0;
                for (k, (a, b)) in base.iter().zip(bits.iter()).enumerate() {
                    if a == b {
                        continue;
                    }
                    changed += 1;
                    let (x, y) = ((k % w) as u8, (k / w) as u8);
                    // 尾巴抬起來，所以上緣會超出原本的框，最多 tail_lift
                    assert!(
                        x >= tx0 && x <= tx1 && y + p.tail_lift >= ty0 && y <= ty1,
                        "{} {} 第 {i} 格動到了尾巴以外的 ({x},{y})",
                        sp.name(),
                        p.state.name()
                    );
                }
                assert!(
                    changed > 30,
                    "{} {} 第 {i} 格只改了 {changed} 個子像素 —— 這種幅度看不出尾巴翹起來",
                    sp.name(),
                    p.state.name()
                );
            }
        }
    }

    #[test]
    fn the_tail_curl_actually_reverses_the_curvature() {
        // 光是「尾尖 y 變小」不夠 —— 整條尾巴平移上去也會通過。
        // 弧：曲率翻轉，根部幾乎不動、越靠尾尖抬得越多。
        // 波：根部要跟著擺，但尾尖過了弧頂還得垂下來 —— 反過來的 S 的
        // 第二個彎。少了它，根部跟著抬就只是整條平移。
        for (sp, p) in all_poses() {
            let Some(up) = p.frames.iter().position(|f| f.tail == 2) else {
                continue;
            };
            let (w, h, down) = rasterize(p, 0);
            let (_, _, raised) = rasterize(p, up);
            let top_in = |bits: &[bool], x0: usize, x1: usize| {
                (0..h).find(|&y| (x0..=x1.min(w - 1)).any(|x| bits[y * w + x]))
            };
            let (tx0, tx1) = (p.tail.0 as usize, p.tail.1 as usize);
            let seg = (tx1 - tx0) / 3;
            let tip_a = top_in(&down, tx0, tx0 + seg).unwrap();
            let tip_b = top_in(&raised, tx0, tx0 + seg).unwrap();
            let root_a = top_in(&down, tx1 - seg, tx1).unwrap();
            let root_b = top_in(&raised, tx1 - seg, tx1).unwrap();
            let tip_lift = tip_a.saturating_sub(tip_b);
            let root_lift = root_a.saturating_sub(root_b);
            assert!(
                tip_lift >= 8,
                "{} {} 的尾尖只抬了 {tip_lift} 個子像素",
                sp.name(),
                p.state.name()
            );
            match p.tail_curve {
                Curve::Arc => assert!(
                    tip_lift >= root_lift * 3,
                    "{} {} 尾尖抬 {tip_lift}、根部抬 {root_lift} —— 這是整條平移，不是勾起來",
                    sp.name(),
                    p.state.name()
                ),
                Curve::Wave => {
                    assert!(
                        root_lift >= 2,
                        "{} {} 的根部沒跟著擺（只抬了 {root_lift}）",
                        sp.name(),
                        p.state.name()
                    );
                    // 只看尾巴框的高度範圍以內：框下面是腳邊的草。
                    let top = |x: usize| (0..=p.tail.3 as usize).find(|&y| raised[y * w + x]);
                    let cols: Vec<usize> = (tx0..=tx1).filter(|&x| top(x).is_some()).collect();
                    let tip_x = cols[0];
                    let peak_x = *cols.iter().min_by_key(|&&x| top(x).unwrap()).unwrap();
                    assert!(
                        peak_x >= tip_x + 3,
                        "{} {} 的弧頂就在尾尖（x {peak_x} vs {tip_x}）—— 沒有第二個彎",
                        sp.name(),
                        p.state.name()
                    );
                    assert!(
                        top(tip_x).unwrap() >= top(peak_x).unwrap() + 2,
                        "{} {} 的尾尖沒有從弧頂垂下來",
                        sp.name(),
                        p.state.name()
                    );
                }
            }
        }
    }

    #[test]
    fn the_curled_tail_stays_in_one_piece() {
        // 相鄰兩欄抬的量差太多會斷成一串點 —— 尾尖收細到只剩一點時特別明顯。
        for (sp, p) in all_poses() {
            for (i, f) in p.frames.iter().enumerate() {
                if f.tail == 0 {
                    continue;
                }
                let (w, _, bits) = rasterize(p, i);
                let (tx0, tx1, y1) = (p.tail.0 as usize, p.tail.1 as usize, p.tail.3 as usize);
                let rows = |x: usize| (0..=y1).filter(|&y| bits[y * w + x]).collect::<Vec<_>>();
                let mut prev: Option<(usize, Vec<usize>)> = None;
                for x in (tx0..=tx1).rev() {
                    let r = rows(x);
                    if r.is_empty() {
                        continue;
                    }
                    if let Some((px, pr)) = &prev {
                        if x + 1 == *px {
                            let touches = r.iter().any(|a| pr.iter().any(|b| a.abs_diff(*b) <= 1));
                            assert!(
                                touches,
                                "{} {} 第 {i} 格：尾巴在 x={x} 斷開",
                                sp.name(),
                                p.state.name()
                            );
                        }
                    }
                    prev = Some((x, r));
                }
            }
        }
    }

    #[test]
    fn the_curl_has_a_halfway_frame_so_it_reads_as_a_movement() {
        // 直接從平的跳到勾起來會像換了一張圖
        for (sp, p) in all_poses() {
            if !p.frames.iter().any(|f| f.tail != 0) {
                continue;
            }
            assert!(
                p.frames.iter().any(|f| f.tail == 1),
                "{} {} 沒有中間過渡的那一格",
                sp.name(),
                p.state.name()
            );
            let (w, h, _) = rasterize(p, 0);
            let tip = |frame: usize| {
                let (_, _, b) = rasterize(p, frame);
                (0..h).find(|&y| {
                    (p.tail.0 as usize..=(p.tail.0 as usize + 8).min(w - 1)).any(|x| b[y * w + x])
                })
            };
            let mid = p.frames.iter().position(|f| f.tail == 1).unwrap();
            let up = p.frames.iter().position(|f| f.tail == 2).unwrap();
            let (d, m, u) = (tip(0).unwrap(), tip(mid).unwrap(), tip(up).unwrap());
            assert!(d > m && m > u, "三格的尾尖高度不是遞增：{d} → {m} → {u}");
        }
    }

    #[test]
    fn only_the_calm_poses_raise_their_tail() {
        // 負載高的時候還在開心搖尾巴，傳達的訊息是錯的 ——
        // 姿態是在報告系統狀態，不是純裝飾。
        for (sp, p) in all_poses() {
            let wags = p.frames.iter().any(|f| f.tail != 0);
            let calm = matches!(p.state, State::Observe | State::Rest);
            if wags {
                assert!(
                    calm,
                    "{} {} 在非安靜狀態下翹尾巴",
                    sp.name(),
                    p.state.name()
                );
                assert_eq!(sp, Species::Fox, "只有狐狸翹尾巴");
            }
        }
        // 而安靜的狐狸真的要會搖 —— 不然這個功能等於沒做
        for st in [State::Observe, State::Rest] {
            let p = pose(Species::Fox, st);
            assert!(
                p.frames.iter().any(|f| f.tail != 0),
                "FOX {} 沒有翹尾巴的格",
                st.name()
            );
        }
    }

    #[test]
    fn the_idle_loop_is_not_a_regular_beat() {
        // still-blink-still-breath 這種規律節奏看起來像 GIF 在循環。
        // 要求：大部分時間靜止，而且動作不是等距分布。
        for (sp, p) in all_poses() {
            let n = p.frames.len();
            assert!(
                n >= 12,
                "{} {} 只有 {n} 格，循環太短",
                sp.name(),
                p.state.name()
            );
            let moving: Vec<usize> = p
                .frames
                .iter()
                .enumerate()
                .filter(|(_, f)| f.breath || f.ears || !f.clear.is_empty())
                .map(|(i, _)| i)
                .collect();
            assert!(
                moving.len() * 2 <= n,
                "{} {} 有動作的格數超過一半，太忙",
                sp.name(),
                p.state.name()
            );
            let gaps: std::collections::HashSet<usize> =
                moving.windows(2).map(|w| w[1] - w[0]).collect();
            assert!(
                gaps.len() > 1,
                "{} {} 的動作是等距分布的，看起來像機械循環",
                sp.name(),
                p.state.name()
            );
        }
    }

    #[test]
    fn every_pose_has_ink_and_at_least_one_frame() {
        for (sp, p) in all_poses() {
            let ink = p
                .art
                .iter()
                .flat_map(|r| r.chars())
                .filter(|c| *c == '#')
                .count();
            // 門檻訂得比「一整塊實心」低很多，因為萃取版**刻意保留**了
            // 原圖內部的分隔線 —— 那些沒被填的格子是線條不是缺陷。
            assert!(
                ink > 480,
                "{} {} 只有 {ink} 個點，畫面上會看不出是什麼",
                sp.name(),
                p.state.name()
            );
            assert!(
                !p.frames.is_empty(),
                "{} {} 沒有動畫格",
                sp.name(),
                p.state.name()
            );
        }
    }

    #[test]
    fn frame_deltas_stay_inside_the_bitmap() {
        for (sp, p) in all_poses() {
            let w = p.art[0].chars().count();
            let h = p.art.len();
            for (i, f) in p.frames.iter().enumerate() {
                for &(x, y) in f.clear.iter().chain(f.set.iter()) {
                    assert!(
                        (x as usize) < w && (y as usize) < h,
                        "{} {} 第 {i} 格的座標 ({x},{y}) 超出 {w}x{h}",
                        sp.name(),
                        p.state.name()
                    );
                }
                // 呼吸與耳動的範圍必須在圖內，而且是有效的區間
                for (label, span) in [("back", p.back), ("ears", p.ears)] {
                    if span == (0, 0) {
                        continue;
                    }
                    assert!(
                        span.0 < span.1 && (span.1 as usize) < w,
                        "{} {} 的 {label} 範圍 {span:?} 不合法",
                        sp.name(),
                        p.state.name()
                    );
                }
            }
        }
    }

    #[test]
    fn rasterizing_any_frame_never_panics_and_keeps_the_size() {
        for (_, p) in all_poses() {
            for f in 0..12 {
                let (w, h, bits) = rasterize(p, f);
                assert_eq!(bits.len(), w * h);
                assert_eq!(h, p.art.len());
            }
        }
    }

    #[test]
    fn half_block_output_only_uses_the_four_block_characters() {
        for (_, p) in all_poses() {
            let (w, h, bits) = rasterize(p, 0);
            for line in to_lines(w, h, &bits) {
                assert!(
                    line.chars().all(|c| QUAD.contains(&c)),
                    "四分格輸出混進了別的字元：{line:?}"
                );
            }
        }
    }

    #[test]
    fn the_ground_line_is_stable_for_the_same_seed() {
        let a = ground(40, 7);
        let b = ground(40, 7);
        assert_eq!(a, b, "同一個 seed 要畫出同一條地面，否則畫面會抖");
        assert_eq!(a.chars().count(), 40);
        assert_ne!(ground(40, 7), ground(40, 99));
    }

    #[test]
    fn captions_are_present_for_every_state() {
        for st in State::ALL {
            let (a, b) = st.caption();
            assert!(!a.is_empty() && !b.is_empty(), "{} 沒有標語", st.name());
            assert!(a.is_ascii(), "標語要能在單色終端顯示");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 依系統狀態挑姿態
// ─────────────────────────────────────────────────────────────────────────

/// 系統現在的「氣氛」。刻意做成一個很小的列舉，而不是讓 mascot 直接讀
/// collector —— 裝飾層不該碰資料層，這條界線跟 UI 不讀 `/proc` 是同一條。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    /// 幾乎沒事做
    Idle,
    /// 正常運轉
    Calm,
    /// 忙但健康
    Busy,
    /// 有東西吃緊
    Strained,
    /// 剛從吃緊恢復
    Recovering,
}

impl Mood {
    pub fn state(self) -> State {
        match self {
            Self::Idle => State::Rest,
            Self::Calm => State::Observe,
            Self::Busy => State::Explore,
            Self::Strained => State::Proceed,
            Self::Recovering => State::Return,
        }
    }
}

/// 依設定與氣氛決定「現在要畫哪一隻、哪個姿態」。
///
/// 回傳 `None` 代表這次不畫吉祥物。
pub fn choose(
    species_setting: &str,
    mode: &str,
    manual_state: &str,
    mood: Mood,
    idle: bool,
) -> Option<(Species, State)> {
    let species = match species_setting {
        "none" => return None,
        "fox" => Species::Fox,
        "deer" => Species::Deer,
        // auto 就是狐狸。
        //
        // 本來讓它依系統狀態換種類，但那很難預期 —— 使用者不知道自己
        // 什麼時候看得到哪一隻，只會覺得「怎麼有時候有有時候沒有」。
        // 換**姿態**才是跟著狀態走的部分，種類固定下來比較好懂。
        _ => Species::Fox,
    };
    let state = match mode {
        "manual" => State::parse(manual_state).unwrap_or(State::Observe),
        // idle 模式只在系統真的閒下來時才出現，其餘時間讓位給資料
        "idle" => {
            if !idle {
                return None;
            }
            State::Rest
        }
        _ => mood.state(),
    };
    Some((species, state))
}

#[cfg(test)]
mod choose_tests {
    use super::*;

    #[test]
    fn none_never_draws_anything() {
        for mode in ["reactive", "manual", "idle"] {
            assert_eq!(choose("none", mode, "observe", Mood::Calm, true), None);
        }
    }

    #[test]
    fn manual_pins_the_state_regardless_of_mood() {
        for mood in [Mood::Idle, Mood::Calm, Mood::Busy, Mood::Strained] {
            let (sp, st) = choose("fox", "manual", "rest", mood, false).unwrap();
            assert_eq!(sp, Species::Fox);
            assert_eq!(st, State::Rest, "manual 模式不該被系統狀態改掉");
        }
    }

    #[test]
    fn reactive_follows_the_mood() {
        for (mood, want) in [
            (Mood::Idle, State::Rest),
            (Mood::Calm, State::Observe),
            (Mood::Busy, State::Explore),
            (Mood::Strained, State::Proceed),
            (Mood::Recovering, State::Return),
        ] {
            let (_, st) = choose("fox", "reactive", "observe", mood, false).unwrap();
            assert_eq!(st, want, "{mood:?} 應該對到 {}", want.name());
        }
    }

    #[test]
    fn idle_mode_stays_out_of_the_way_while_the_machine_is_working() {
        assert_eq!(choose("fox", "idle", "observe", Mood::Busy, false), None);
        assert!(choose("fox", "idle", "observe", Mood::Idle, true).is_some());
    }

    #[test]
    fn auto_always_picks_the_same_species() {
        // 種類不該跟著系統狀態變 —— 使用者會搞不清楚自己什麼時候看得到誰。
        // 跟著狀態走的是姿態，不是種類。
        for mood in [
            Mood::Idle,
            Mood::Calm,
            Mood::Busy,
            Mood::Strained,
            Mood::Recovering,
        ] {
            let (sp, _) = choose("auto", "reactive", "observe", mood, false).unwrap();
            assert_eq!(sp, Species::Fox, "{mood:?} 挑到了不同的種類");
        }
    }

    #[test]
    fn manual_state_falls_back_instead_of_panicking() {
        let (_, st) = choose("deer", "manual", "nonsense", Mood::Calm, false).unwrap();
        assert_eq!(st, State::Observe);
    }
}
