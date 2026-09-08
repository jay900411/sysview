//! 彩蛋：終端機裡的恐龍跑酷。
//!
//! # 數值來自哪裡
//!
//! 手感、進度、障礙物組成全部對照 **Chromium 官方實作**（`components/
//! neterror/resources/dino_game/`，BSD），不是憑感覺調的，也不是抄部落格上
//! 那些「500 分出現翼龍」的傳聞。對照表與換算方式寫在 `docs/dino.md`。
//!
//! 沒有複製任何一行程式碼：拿的是**行為**（速度曲線、跳躍滯空、障礙物
//! 尺寸比例、解鎖條件、日夜切換的距離）。剪影是照原版的像素畫**一塊一塊
//! 抓下來**的（它的像素畫本來就是 2×2 px 一塊，一塊對一個 braille 點）。
//!
//! # 兩套單位
//!
//! * **Chrome 單位**：`speed` 6→13 px/frame、`distance` px。障礙物的間距
//!   公式、群組門檻、翼龍解鎖速度、分數換算全部直接用這一套，這樣才能跟
//!   原版對得起來。
//! * **子像素**：braille 一格 2×4 點，恐龍 22×24 點。畫面上的東西用這一套。
//!
//! 換算只有兩個常數（[`PX_X`] / [`PX_Y`]），集中在一個地方。
//!
//! # 這個彩蛋碰到什麼
//!
//! 不碰 collector、不碰 privilege、不讀 `/proc`、不開檔案、不連網路、
//! 不 spawn 行程。遊戲關掉時 `App::next_game_tick_in` 回傳一小時，
//! 主迴圈一次都不會為它醒來。

use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::app::Key;
use crate::theme::Theme;
use crate::ui::format;

/// Chromium 的原始設定值。名字刻意跟原始碼一樣，方便對照。
mod chrome {
    /// 起始速度（px/frame @60fps）
    pub const SPEED: f32 = 6.0;
    /// 速度上限
    pub const MAX_SPEED: f32 = 13.0;
    /// 每格加速度
    pub const ACCELERATION: f32 = 0.001;
    /// 障礙物間距係數
    pub const GAP_COEFFICIENT: f32 = 0.6;
    /// 間距上限倍率
    pub const MAX_GAP_COEFFICIENT: f32 = 1.5;
    /// 一組最多幾個
    pub const MAX_OBSTACLE_LENGTH: u32 = 3;
    /// 同一種最多連續幾次
    pub const MAX_OBSTACLE_DUPLICATION: usize = 2;
    /// 每隔多少「分數」翻轉日夜
    pub const INVERT_DISTANCE: u32 = 700;
    /// 距離 → 分數
    pub const DISTANCE_COEFFICIENT: f32 = 0.025;
    /// 開場多久之內不出障礙物（毫秒）
    pub const CLEAR_TIME: f32 = 3000.0;
    /// 死掉之後多久才接受「跳躍鍵 = 重來」
    pub const GAMEOVER_CLEAR_TIME: f32 = 750.0;
    /// 恐龍尺寸（px），用來換算比例
    pub const TREX_W: f32 = 44.0;
    #[cfg_attr(not(test), allow(dead_code))]
    pub const TREX_H: f32 = 47.0;
}

/// 一個 Chrome px 等於幾個子像素。
///
/// 由恐龍的尺寸定出來：Chrome 的恐龍 44×47 px，而它的像素畫本來就是
/// 2×2 px 一塊，所以剪影照**塊**抓下來就是 22×24 —— 一塊對一個 braille 點，
/// 沒有縮放、沒有重畫。場地高度也照同一個比例（Chrome 是 150/47 = 3.19 個恐龍高）。
/// 所有長度都照這個比例換算，所以「仙人掌相對恐龍多高」跟原版一樣。
const PX_X: f32 = 22.0 / chrome::TREX_W;
#[cfg_attr(not(test), allow(dead_code))]
const PX_Y: f32 = 24.0 / chrome::TREX_H;

/// 一格多久：60 fps，跟 Chrome 一樣，也跟螢幕的更新率對齊。
///
/// 早期是 33 fps（30 毫秒），理由是「每一格畫面移動多少」跟原版同量級。
/// 但 33 不整除 60：在 60 Hz 的螢幕上一格畫面有時停一個更新週期、有時
/// 停兩個，這種忽快忽慢比較低的幀率還容易看出來（judder），使用者就說
/// 「偏卡」。60 fps 每格移動畫布寬的 1%，跟原版一樣；位元組多一倍、
/// CPU 多一個百分點（量過，見 docs/dino.md）。輸出端塞住時主迴圈自己
/// 降回 30 fps —— 正好一半，仍然對齊 —— 見 `main.rs` 的 `Pacer`。
///
/// 主迴圈平常把背景重畫節流在 25 fps；遊戲開著時會改用這個值。
pub const FRAME: Duration = Duration::from_micros(16_667);

/// 遊戲區至少要這麼大，不然只顯示一行說明。
pub const MIN_W: u16 = 40;
pub const MIN_H: u16 = 8;

/// 場地寬度（字元格）。中間浮一塊，不是佔滿終端機。
///
/// Chrome 的畫布 600 px、恐龍 44 寬、起點 x=50：恐龍佔畫布寬的 7.3%，
/// 從障礙物出現到撞上有 **1.41 秒**可以反應。要完全對上得要 150 欄 ——
/// 那已經是整個終端機的寬度，不再是「中間浮一塊」。130 欄（260 子像素）
/// 是折衷：恐龍佔 8.5%、靠左站在 x=10，反應時間 **1.27 秒**（站在 x=26 是 1.18 秒）。
///
/// 早期是 72 欄：反應時間只有 0.84 秒。恐龍不是畫太大，是**跑道太短** ——
/// 障礙物一出現就已經在眼前了。
pub const FIELD_W: u16 = 130;
pub const FIELD_H: u16 = 19;

/// 次像素解析度：braille 一格是 2 欄 × 4 列。
const SX: i32 = 2;
const SY: i32 = 4;

/// 恐龍站在哪一欄（子像素）。往左靠：Chrome 是 50/600（8%），這裡 10/260（4%）——
/// 跑道短，多留給反應時間。Chrome 是 50/600 = 8%；這裡故意更靠左（10/260 = 4%）——
/// 終端機一格就是 5 個子像素，反應的時間顆粒度比原版粗得多，
/// 多給一點跑道是為了補這個差距。
const DINO_X: i32 = 10;

/// 跳躍：起跳速度與重力（子像素 / 秒）。
///
/// 不是手調的。把 Chromium 的跳躍演算法原封不動跑一次，量到：
/// **滯空 0.583 秒、最高點 91 px = 1.94 個恐龍高**。
/// 恐龍是 24 個子像素高，所以最高點 = 1.94 × 24 = 46.6 子像素，
/// `v0 = 2·apex/(t/2) = 320`、`g = v0/(t/2) = 1097`。
const JUMP_V0: f32 = 320.0;
const GRAVITY: f32 = 1097.0;
/// 空中按 ↓ 的加速倍率。Chrome 的 speedDropCoefficient 就是 3。
const SPEED_DROP: f32 = 3.0;
/// 一次跳躍的滯空時間（秒）。由 `JUMP_V0` 與 `GRAVITY` 決定，
/// 這裡寫出來是因為間距下限要用到。
const AIRTIME: f32 = 2.0 * JUMP_V0 / GRAVITY;

/// 一次 ↓ 讓恐龍蹲多久。
///
/// 終端機**收不到放開鍵**（實測：crossterm 在這裡只送 `Press`，
/// 沒有 `Release` 也沒有 `Repeat`），所以不能照瀏覽器的 keydown/keyup
/// 模型。改成「按一下蹲一段時間，再按就延長」：
/// 按住時鍵盤自動重複會一直延長，點一下也會蹲得夠久讓人看得見。
///
/// 一次 ↓ 讓恐龍蹲多久。終端機收不到放開鍵，所以蹲下必須自己撐一段時間；
/// 按住時作業系統的自動重複會一直送 ↓ 進來，每一下都把時間往後延。
///
/// 撐多久，決定的不是「什麼時候放開」，而是**兩個 ↓ 之間最長會隔多久**：
///
/// * 第一下之後要等作業系統的起始延遲（X11 660 ms、GNOME / Windows 500、
///   macOS 預設 375、最慢 1.8 s），重複才會開始。
/// * 重複開始之後也不保證準時：按鍵是經過終端機、ssh、VS Code 的 pty host
///   才到這裡的，機器忙起來那條管線會卡個幾百毫秒再一次補送。真 PTY 量到：
///   每 30 ms 一個 ↓、管線卡 200 ms，恐龍就站了 100 ms —— 使用者看到的
///   「有時候閃一幀」就是這個，正好在翼龍底下就死了。
///
/// 早期用 400 / 750 ms；後來試過「跟著重複間隔縮短」（放開後能更快站起來），
/// 但那把容忍的空隙縮到 120 ms，管線一頓就閃。程式沒有辦法分辨「管線卡住」
/// 和「真的放開」，唯一的槓桿就是這個時間；而蹲久一點沒有任何壞處（蹲著照樣
/// 能跳、蹲不會撞到本來不會撞的東西），所以取 2 秒：起始延遲與常見的卡頓
/// 都蓋得過，放開之後最多蹲兩秒。
const DUCK_HOLD: Duration = Duration::from_millis(2000);
/// 畫面上站起來之後，碰撞箱再多算蹲的時間。管線卡超過 [`DUCK_HOLD`] 那一格
/// 畫面會閃一下，但至少不該因此死掉。
const DUCK_HIT_GRACE: Duration = Duration::from_millis(1000);

// ── 剪影 ────────────────────────────────────────────────────────────────
//
// 照 Chrome 的 T-Rex 一塊一塊抓下來：44×47 px = 22×24 塊（原版的像素畫
// 就是 2×2 px 一塊；47 是奇數，最後半塊算一整塊）。方頭在右上、上面兩個
// 角各缺一塊、一格白的眼睛、下顎的缺口、往左上翹的細尾巴、尾巴與脖子之間
// 背部的凹陷、小手、兩條腿；跑步兩格換腳，抬起來的那隻腿縮短、腳掌往前
// 伸。撞到：眼睛放大一圈成 3×3 的洞、中間留一點。蹲下：59×25 px 抓成
// 29×13 塊 —— 尾巴翹在左上、背部平、頭跟背同高、下顎在肚子的高度。
// 早期自己照比例畫的版本把背部畫平、尾巴併進身體，一眼就看得出不對。

#[rustfmt::skip]
const RUN_A: [&str; 24] = [
    "............#########.",
    "...........##.########",
    "...........###########",
    "...........###########",
    "...........###########",
    "...........###########",
    "...........#####......",
    "...........########...",
    "#.........#####.......",
    "#.........#####.......",
    "#.......#######.......",
    "##.....##########.....",
    "###...#########.#.....",
    "###############.......",
    "###############.......",
    ".##############.......",
    "..############........",
    ".....########.........",
    ".....#######..........",
    ".....####.##..........",
    ".....####.###.........",
    ".....###..............",
    ".....#................",
    ".....###..............",
];
#[rustfmt::skip]
const RUN_B: [&str; 24] = [
    "............#########.",
    "...........##.########",
    "...........###########",
    "...........###########",
    "...........###########",
    "...........###########",
    "...........#####......",
    "...........########...",
    "#.........#####.......",
    "#.........#####.......",
    "#.......#######.......",
    "##.....##########.....",
    "###...#########.#.....",
    "###############.......",
    "###############.......",
    ".##############.......",
    "..############........",
    ".....########.........",
    ".....#######..........",
    ".....####.##..........",
    ".....####.##..........",
    "......###..#..........",
    "...........#..........",
    "...........##.........",
];
/// 蹲下：Chrome 是 59×25 px → 30×13 塊。
///
/// 不是把頭抬高的蛇 —— 是壓低身體、頭往前伸的鱷魚：頭頂跟身體上緣齊平，
/// 身體是厚的一整塊，尾巴仍是左上那個小三角，腿變短。
#[rustfmt::skip]
const DUCK: [&str; 13] = [
    "#...................########.",
    "###....#########...##########",
    "#####################.#######",
    ".############################",
    "..###########################",
    "...##########################",
    "....####################.....",
    ".....############...#######..",
    "......###########............",
    "......###......#.............",
    "......###......##............",
    "......##.....................",
    "......###....................",
];
/// 撞到之後：跟原版一樣，眼睛從一格空洞變成大一圈的實心眼、中間一個小點。
/// 兩腳都在地上。
#[rustfmt::skip]
const CRASH: [&str; 24] = [
    "............#########.",
    "...........#...#######",
    "...........#.#.#######",
    "...........#...#######",
    "...........###########",
    "...........###########",
    "...........#####......",
    "...........########...",
    "#.........#####.......",
    "#.........#####.......",
    "#.......#######.......",
    "##.....##########.....",
    "###...#########.#.....",
    "###############.......",
    "###############.......",
    ".##############.......",
    "..############........",
    ".....########.........",
    ".....#######..........",
    ".....####.##..........",
    ".....####.##..........",
    ".....###...#..........",
    ".....#.....#..........",
    ".....###...##.........",
];

/// 小仙人掌：Chrome 17×35 px → 9×18 塊。莖 3 塊寬，兩隻手臂高度不同。
#[rustfmt::skip]
const CACTUS_S: [&str; 18] = [
    "....#....",
    "...###...",
    "...###...",
    "...###..#",
    "#..###..#",
    "#..###..#",
    "#..###..#",
    "#..###..#",
    "##.###.##",
    ".#######.",
    "..#####..",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
    "...###...",
];
/// 大仙人掌：Chrome 25×50 px → 13×25 塊。比恐龍高，跟原版一樣。
#[rustfmt::skip]
const CACTUS_L: [&str; 25] = [
    ".....###.....",
    "....#####....",
    "....#####....",
    "....#####....",
    "....#####....",
    "#...#####....",
    "##..#####..##",
    "##..#####..##",
    "##..#####..##",
    "##..#####..##",
    "##..#####..##",
    "##..#####..##",
    "##..#####..##",
    "###.#####.###",
    ".#########.#.",
    "..#########..",
    "...#######...",
    "....#####....",
    "....#####....",
    "....#####....",
    "....#####....",
    "....#####....",
    "....#####....",
    "....#####....",
    "....#####....",
];
/// 翼龍：Chrome 46×40 px → 23×20 塊。嘴朝左（朝著恐龍飛），兩格拍翅。
#[rustfmt::skip]
const BIRD_UP: [&str; 20] = [
    "..........##...........",
    ".........###...........",
    "........####...........",
    ".......#####...........",
    "......######...........",
    ".....#######...........",
    "....########...........",
    "...#########...........",
    "..##########...........",
    "###############........",
    "##################.....",
    ".######################",
    "...######.###########..",
    "......####.............",
    "......###..............",
    ".......##..............",
    ".......................",
    ".......................",
    ".......................",
    ".......................",
];
#[rustfmt::skip]
const BIRD_DOWN: [&str; 20] = [
    ".......................",
    ".......................",
    ".......................",
    ".......................",
    ".......................",
    ".......................",
    ".......................",
    "......##...............",
    "......###..............",
    "###############........",
    "##################.....",
    ".######################",
    "..###########.#######..",
    "..###########..........",
    "...##########..........",
    "....#########..........",
    ".....########..........",
    "......#######..........",
    ".......######..........",
    "........#####..........",
];

/// 障礙物種類。尺寸、間距、解鎖速度全部照 Chrome 的 `obstacleTypes`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// 小仙人掌。Chrome：multipleSpeed 4、minGap 120、minSpeed 0
    CactusSmall,
    /// 大仙人掌。Chrome：multipleSpeed 7、minGap 120、minSpeed 0
    CactusLarge,
    /// 翼龍。Chrome：minSpeed 8.5、minGap 150、不成群、三種高度
    Pterodactyl(u8),
}

impl Kind {
    /// 這一種在幾速以上才會出現（Chrome 單位）。
    fn min_speed(self) -> f32 {
        match self {
            Self::Pterodactyl(_) => 8.5,
            _ => 0.0,
        }
    }
    /// 幾速以上才允許成群。
    fn multiple_speed(self) -> f32 {
        match self {
            Self::CactusSmall => 4.0,
            Self::CactusLarge => 7.0,
            Self::Pterodactyl(_) => 999.0,
        }
    }
    /// Chrome 的 minGap（px）。
    fn min_gap_px(self) -> f32 {
        match self {
            Self::Pterodactyl(_) => 150.0,
            _ => 120.0,
        }
    }
    /// 單體寬度（子像素）。
    fn unit_w(self) -> i32 {
        match self {
            Self::CactusSmall => CACTUS_S[0].len() as i32,
            Self::CactusLarge => CACTUS_L[0].len() as i32,
            Self::Pterodactyl(_) => BIRD_UP[0].len() as i32,
        }
    }
    fn height(self) -> i32 {
        match self {
            Self::CactusSmall => CACTUS_S.len() as i32,
            Self::CactusLarge => CACTUS_L.len() as i32,
            Self::Pterodactyl(_) => BIRD_UP.len() as i32,
        }
    }
    /// 碰撞範圍（離 `base` 幾個子像素）。
    ///
    /// 仙人掌就是整棵。翼龍不是：Chrome 的翼龍碰撞箱只有身體那一段
    /// （sprite 40 px 高，碰撞箱在 y 8..27），翅膀不算。照抄整個外框的話
    /// 「明明從翅膀下面過去卻死了」。換算後是離底邊 7..16 子像素。
    fn hit_range(self) -> (i32, i32) {
        match self {
            Self::Pterodactyl(_) => (7, 16),
            k => (0, k.height()),
        }
    }
    /// 底邊離地幾個子像素。
    ///
    /// Chrome 的翼龍 yPos 是 [100, 75, 50]，地面在 140，所以離地
    /// 0 / 25 / 50 px。換算後是 0 / 13 / 26 子像素：
    /// 最低的要跳、中間的要蹲、最高的直接跑過去。
    fn base(self) -> i32 {
        match self {
            Self::Pterodactyl(0) => 0,
            Self::Pterodactyl(1) => 13,
            Self::Pterodactyl(_) => 26,
            _ => 0,
        }
    }
    fn art(self, t: f32) -> &'static [&'static str] {
        match self {
            Self::CactusSmall => &CACTUS_S,
            Self::CactusLarge => &CACTUS_L,
            // Chrome 的翼龍是 6 fps 兩格
            Self::Pterodactyl(_) => {
                if (t * 6.0) as i64 % 2 == 0 {
                    &BIRD_UP
                } else {
                    &BIRD_DOWN
                }
            }
        }
    }
    fn family(self) -> u8 {
        match self {
            Self::CactusSmall => 0,
            Self::CactusLarge => 1,
            Self::Pterodactyl(_) => 2,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Obs {
    /// 左緣（子像素）
    x: f32,
    kind: Kind,
    /// 幾個並排（仙人掌才會 > 1）
    size: i32,
    /// 這一個之後要隔多遠才生下一個（子像素）
    gap: f32,
    /// 下一個生出來了沒
    spawned: bool,
}

impl Obs {
    fn width(&self) -> i32 {
        self.kind.unit_w() * self.size
    }
}

/// xorshift64\*。彩蛋不需要 CSPRNG，但需要「每次玩不一樣」而且
/// 「測試給同一個種子就得到同一串」。
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn from_seed(seed: u64) -> Self {
        Self(seed | 1)
    }
    /// 正式遊玩用的種子。
    ///
    /// 取自 `RandomState` —— 標準庫的 HashMap 用它防雜湊碰撞攻擊，
    /// 每個行程的值由作業系統的亂數來源決定。這樣就不必為了一個彩蛋
    /// 多一個相依，也不必自己開 `/dev/urandom`（這個模組不碰檔案）。
    /// 用時間當種子是不行的：同一秒內開兩局會拿到同一串障礙物。
    pub fn from_entropy() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(0x9E37_79B9_7F4A_7C15);
        Self::from_seed(h.finish())
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// `[lo, hi]` 之間的整數（含兩端），跟 Chrome 的 `getRandomNum` 一樣。
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % ((hi - lo + 1) as u64)) as i64
    }
    fn frange(&mut self, lo: f32, hi: f32) -> f32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % 10_000) as f32 / 10_000.0 * (hi - lo)
    }
}

/// 死掉之後要畫在場地上的東西：榜、名字輸入框、結果訊息。
///
/// 由 App 提供 —— 遊戲本身不碰檔案，也不知道榜存在哪裡。
pub struct OverPanel<'a> {
    pub top: &'a [crate::scoreboard::Entry],
    /// 有共用榜嗎（沒有的話標題會說只有自己）。
    pub shared: bool,
    /// 正在輸入的名字；`Some` = 輸入框開著。
    pub typed: Option<&'a str>,
    /// 送出 / 略過之後的一句話。
    pub note: Option<&'a str>,
}

/// 一局遊戲。
#[derive(Debug)]
pub struct Dino {
    /// 場地寬度（子像素）。resize 之後跟著變。
    ///
    /// 是 `Cell` 因為只有 `render` 知道場地多寬，而 `render` 拿到的是
    /// `&self` —— UI 那一層整條路徑都是 `&App`。
    w: std::cell::Cell<f32>,
    /// Chrome 單位的速度（6 → 13）。所有進度都由它決定。
    speed: f32,
    /// Chrome 單位的距離（px），分數由它換算。
    distance: f32,
    /// 腳底離地幾個子像素
    y: f32,
    vy: f32,
    /// 蹲到什麼時候。過期就自動站起來 —— 終端機沒有放開鍵。
    duck_until: Option<Instant>,
    /// 空中按 ↓ 的快速下墜
    speed_drop: bool,
    obstacles: Vec<Obs>,
    history: Vec<u8>,
    /// 測試用：生出來的東西照順序記下來，用來驗證「同種子同序列」。
    #[cfg(test)]
    spawn_log: Vec<(u8, i32)>,
    best: u32,
    over_at: Option<Instant>,
    rng: Rng,
    started: Instant,
    /// 上一次推進物理的時刻（delta-time 用）
    last: Instant,
    /// 上一次回報「該重畫了」的時刻
    last_frame: Instant,
}

impl Dino {
    pub fn new(now: Instant, best: u32) -> Self {
        Self::with_rng(now, best, Rng::from_entropy())
    }

    /// 測試用：給定種子，障礙物序列就是可重現的。
    pub fn with_rng(now: Instant, best: u32, rng: Rng) -> Self {
        Self {
            w: std::cell::Cell::new(140.0),
            speed: chrome::SPEED,
            distance: 0.0,
            y: 0.0,
            vy: 0.0,
            duck_until: None,
            speed_drop: false,
            obstacles: Vec::new(),
            history: Vec::new(),
            #[cfg(test)]
            spawn_log: Vec::new(),
            best,
            over_at: None,
            rng,
            started: now,
            last: now,
            last_frame: now,
        }
    }

    /// Chrome 的距離→分數換算。
    pub fn score(&self) -> u32 {
        (self.distance * chrome::DISTANCE_COEFFICIENT).round() as u32
    }
    pub fn best(&self) -> u32 {
        self.best.max(self.score())
    }
    pub fn is_over(&self) -> bool {
        self.over_at.is_some()
    }
    pub fn last_tick(&self) -> Instant {
        self.last_frame
    }
    /// 給預覽 / 截圖用的簡易自動玩家：眼前會撞到就跳，中高度的鳥就蹲。
    ///
    /// 不是遊戲的一部分，也不是測試用的那個前瞻搜尋 —— 這只是為了產生
    /// 「正在玩」的畫面，不然截圖永遠是 GAME OVER。
    pub fn autopilot(&mut self, now: Instant) {
        if self.is_over() {
            self.on_key(Key::Char('r'), now + Duration::from_secs(1));
            return;
        }
        let (dx0, dx1, ..) = self.hitbox(now);
        let sp = self.speed_sub();
        let Some(o) = self
            .obstacles
            .iter()
            .filter(|o| o.x + o.width() as f32 > dx0)
            .min_by(|a, b| a.x.total_cmp(&b.x))
            .copied()
        else {
            return;
        };
        let top = (o.kind.base() + o.kind.hit_range().1) as f32;
        let disc = JUMP_V0 * JUMP_V0 - 2.0 * GRAVITY * top;
        let rise = if disc > 0.0 {
            (JUMP_V0 - disc.sqrt()) / GRAVITY
        } else {
            0.3
        };
        match o.kind {
            Kind::Pterodactyl(1) if o.x < dx1 + sp * 0.25 => {
                self.on_key(Key::Down, now);
            }
            Kind::Pterodactyl(1) | Kind::Pterodactyl(2) => {}
            _ => {
                if self.y <= 0.0 && o.x < dx1 + rise * sp + sp * 0.06 {
                    self.on_key(Key::Char(' '), now);
                }
            }
        }
    }

    /// 場上有沒有翼龍（截圖 / 預覽用）。
    pub fn obstacles_have_bird(&self) -> bool {
        self.obstacles
            .iter()
            .any(|o| matches!(o.kind, Kind::Pterodactyl(_)))
    }

    /// 現在是不是夜晚。Chrome 每 700 分翻轉一次。
    pub fn is_night(&self) -> bool {
        (self.score() / chrome::INVERT_DISTANCE) % 2 == 1
    }
    /// 剛站起來不久：碰撞箱還算蹲（見 [`DUCK_HIT_GRACE`]）。
    fn duck_hitbox(&self, now: Instant) -> bool {
        !self.is_over()
            && self.y <= 0.0
            && self.duck_until.is_some_and(|t| t + DUCK_HIT_GRACE > now)
    }

    fn ducking(&self, now: Instant) -> bool {
        // 撞到之後畫的是站著的撞擊圖，位置也得回到站著的地方 —— 不然蹲著
        // 撞上的那一格，恐龍會往後跳三格半（蹲姿是往後長的）。
        !self.is_over() && self.duck_until.is_some_and(|t| t > now) && self.y <= 0.0
    }

    /// 速度換算成子像素 / 秒。
    fn speed_sub(&self) -> f32 {
        self.speed * 60.0 * PX_X
    }

    /// 按鍵。回傳 true 代表狀態有變、值得立刻重畫。
    pub fn on_key(&mut self, key: Key, now: Instant) -> bool {
        let jump = matches!(
            key,
            Key::Char(' ') | Key::Up | Key::Char('w') | Key::Char('k')
        );
        let down = matches!(key, Key::Down | Key::Char('s') | Key::Char('j'));
        if let Some(dead) = self.over_at {
            // 死掉之後：只有明確的重來鍵有效。
            //
            // 早期版本是「除了 Esc 之外任何鍵都重來」，於是 Help 上寫的
            // `r 重來` 毫無意義，而且手一滑就重開一局。
            let grace =
                now.duration_since(dead).as_secs_f32() * 1000.0 >= chrome::GAMEOVER_CLEAR_TIME;
            if matches!(key, Key::Char('r') | Key::Enter) || (jump && grace) {
                self.restart(now);
                return true;
            }
            return false;
        }
        if jump {
            if self.y <= 0.0 {
                // Chrome：起跳速度隨速度微調（快的時候跳得略低）
                self.vy = JUMP_V0 - self.speed_sub() * 0.02;
                self.duck_until = None;
                self.speed_drop = false;
                // 立刻推一小步，這樣「按下去」跟「看到牠離地」是同一幀。
                // 純粹等下一個物理步的話，最壞要等一整格才看得到反應。
                self.y = 1.0;
                return true;
            }
            return false;
        }
        if down {
            if self.y > 0.0 {
                // 空中 ↓ = 快速下墜（Chrome 的 speed drop）
                self.speed_drop = true;
                self.vy = -(self.vy.abs().max(JUMP_V0 * 0.35)) * SPEED_DROP;
            }
            // 地面 ↓ = 蹲。落地時 speed drop 也會變成蹲（跟 Chrome 一樣）。
            self.duck_until = Some(now + DUCK_HOLD);
            return true;
        }
        false
    }

    /// 測試用：複製一份現在的狀態（`Dino` 沒有 `Clone`，因為它不該被複製）。
    #[cfg(test)]
    fn clone_for_test(&self) -> Self {
        Self {
            w: std::cell::Cell::new(self.w.get()),
            speed: self.speed,
            distance: self.distance,
            y: self.y,
            vy: self.vy,
            duck_until: self.duck_until,
            speed_drop: self.speed_drop,
            obstacles: self.obstacles.clone(),
            history: self.history.clone(),
            spawn_log: self.spawn_log.clone(),
            best: self.best,
            over_at: self.over_at,
            rng: self.rng.clone(),
            started: self.started,
            last: self.last,
            last_frame: self.last_frame,
        }
    }

    fn restart(&mut self, now: Instant) {
        self.best = self.best.max(self.score());
        self.speed = chrome::SPEED;
        self.distance = 0.0;
        self.y = 0.0;
        self.vy = 0.0;
        self.duck_until = None;
        self.speed_drop = false;
        self.obstacles.clear();
        self.history.clear();
        #[cfg(test)]
        self.spawn_log.clear();
        self.over_at = None;
        self.started = now;
        self.last = now;
    }

    /// 推進到 `now`。回傳 true 代表畫面需要重畫。
    ///
    /// 物理是 **delta-time** 的，畫面節奏是另一回事：手感不會因為
    /// 更新率調整而改變，而按鍵造成的移動在下一幀就看得到。
    pub fn tick(&mut self, now: Instant) -> bool {
        let dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        // 卡住之後不要一次補跑一大步（會直接穿過障礙物）
        let dt = dt.clamp(0.0, 0.1);
        if self.over_at.is_none() && dt > 0.0 {
            self.advance(dt, now);
        }
        if now.duration_since(self.last_frame) >= FRAME {
            self.last_frame = now;
            return true;
        }
        false
    }

    fn advance(&mut self, dt: f32, now: Instant) {
        let frames = dt * 60.0; // Chrome 的一格

        // 速度與距離走 Chrome 的公式
        if self.speed < chrome::MAX_SPEED {
            self.speed = (self.speed + chrome::ACCELERATION * frames).min(chrome::MAX_SPEED);
        }
        self.distance += self.speed * frames;

        // 障礙物往左
        let dx = self.speed_sub() * dt;
        for o in &mut self.obstacles {
            o.x -= dx;
        }
        self.obstacles.retain(|o| o.x + o.width() as f32 > -4.0);
        self.spawn(now);

        // 跳躍
        if self.vy != 0.0 || self.y > 0.0 {
            self.y += self.vy * dt;
            self.vy -= GRAVITY * dt * if self.speed_drop { SPEED_DROP } else { 1.0 };
            if self.y <= 0.0 {
                self.y = 0.0;
                self.vy = 0.0;
                if self.speed_drop {
                    // Chrome：落地時還按著 ↓ 就直接變成蹲
                    self.speed_drop = false;
                    self.duck_until = Some(now + DUCK_HOLD);
                }
            }
        }

        if self.collides(now) {
            self.over_at = Some(now);
            self.best = self.best.max(self.score());
        }
    }

    /// Chrome 的生成規則：最後一個障礙物右緣 + gap 進到畫面內就生下一個。
    fn spawn(&mut self, now: Instant) {
        if now.duration_since(self.started).as_secs_f32() * 1000.0 < chrome::CLEAR_TIME {
            return;
        }
        let w = self.w.get();
        let need = match self.obstacles.last() {
            None => true,
            Some(last) => !last.spawned && last.x + last.width() as f32 + last.gap < w,
        };
        if !need {
            return;
        }
        if let Some(last) = self.obstacles.last_mut() {
            last.spawned = true;
        }
        // 抽種類，抽到不合格（速度不夠 / 連續太多次同一種）就重抽。
        // 跟 Chrome 一樣是重抽而不是加權，所以解鎖之後也不會變成一直出鳥。
        let mut kind = Kind::CactusSmall;
        for _ in 0..8 {
            let k = match self.rng.range(0, 2) {
                0 => Kind::CactusSmall,
                1 => Kind::CactusLarge,
                _ => Kind::Pterodactyl(self.rng.range(0, 2) as u8),
            };
            let dup = self.history.len() >= chrome::MAX_OBSTACLE_DUPLICATION
                && self.history.iter().all(|h| *h == k.family());
            if self.speed >= k.min_speed() && !dup {
                kind = k;
                break;
            }
        }
        // 群組：Chrome 抽 1..=3，速度不夠就退回 1
        let mut size = self.rng.range(1, chrome::MAX_OBSTACLE_LENGTH as i64) as i32;
        if size > 1 && kind.multiple_speed() > self.speed {
            size = 1;
        }
        let width_px = (kind.unit_w() * size) as f32 / PX_X;
        let min_gap = width_px * self.speed + kind.min_gap_px() * chrome::GAP_COEFFICIENT;
        let gap_px = self
            .rng
            .frange(min_gap, min_gap * chrome::MAX_GAP_COEFFICIENT);
        // 間距下限：至少一次跳躍的水平距離再多一點。
        //
        // 這是**刻意偏離** Chrome 的地方。Chrome 的公式允許間距小於一次
        // 跳躍的長度，玩家得提早起跳才過得去 —— 在 600 px 的畫布上那是
        // 可以練的。我們一格是 5 個子像素（約 0.4 個恐龍寬），時間顆粒度
        // 粗得多，同樣的間距會變成 frame-perfect 才過得去。
        // 加一條下限之後，「看到就跳」永遠來得及。
        let jump_len = AIRTIME * self.speed_sub() / PX_X;
        let gap_px = gap_px.max(jump_len * 1.15);
        self.obstacles.push(Obs {
            x: w,
            kind,
            size,
            gap: gap_px * PX_X,
            spawned: false,
        });
        #[cfg(test)]
        self.spawn_log.push((kind.family(), size));
        self.history.insert(0, kind.family());
        self.history.truncate(chrome::MAX_OBSTACLE_DUPLICATION);
    }

    /// 蹲下時剪影畫在哪一欄。
    ///
    /// Chrome 的蹲下剪影比站著寬（59 vs 44 px），而且是從同一個 x 畫起 ——
    /// 頭往**前**伸 17 px，碰撞箱也跟著往前。在原版那是可以練的；在終端機
    /// 這種粗顆粒的畫面上，「蹲下反而更容易撞到」只會讓人以為壞了。
    /// 所以頭留在原位、多出來的長度往**後**伸：前緣不動。
    fn duck_x() -> i32 {
        DINO_X - (DUCK[0].len() as i32 - RUN_A[0].len() as i32)
    }

    /// 恐龍現在的碰撞範圍（子像素）。
    fn hitbox(&self, now: Instant) -> (f32, f32, f32, f32) {
        // 縮一圈：剪影邊緣有空白，用外框判定會誤判成撞到。
        // 碰撞箱用的是「蹲的保留期」，比畫面上的蹲多撐一段。
        if self.duck_hitbox(now) {
            let x = Self::duck_x() as f32 + 2.0;
            let w = DUCK[0].len() as f32 - 4.0;
            (x, x + w, self.y, self.y + DUCK.len() as f32 - 1.0)
        } else {
            let x = DINO_X as f32 + 2.0;
            let w = RUN_A[0].len() as f32 - 4.0;
            (x, x + w, self.y, self.y + RUN_A.len() as f32 - 2.0)
        }
    }

    fn collides(&self, now: Instant) -> bool {
        let (dx0, dx1, dy0, dy1) = self.hitbox(now);
        self.obstacles.iter().any(|o| {
            let (ox0, ox1) = (o.x + 1.0, o.x + o.width() as f32 - 1.0);
            let (lo, hi) = o.kind.hit_range();
            let base = o.kind.base() as f32;
            let (oy0, oy1) = (base + lo as f32, base + hi as f32);
            dx0 < ox1 && ox0 < dx1 && dy0 < oy1 && oy0 < dy1
        })
    }

    /// 把遊戲畫進 `area`（已經是 overlay 的內容區）。
    pub fn render(
        &self,
        buf: &mut Buffer,
        area: Rect,
        theme: &Theme,
        now: Instant,
        panel: Option<&OverPanel<'_>>,
    ) {
        use crate::ui::widgets::braille::{braille_char, DOTS};

        if area.width < MIN_W || area.height < MIN_H {
            let msg = format!("視窗太小，恐龍需要至少 {MIN_W}×{MIN_H}");
            buf.set_string(
                area.x,
                area.y + area.height / 2,
                format::truncate(&msg, area.width as usize),
                theme.dim_style(),
            );
            return;
        }

        // 日夜：只翻轉遊戲畫布，不動 sysview 的主題。
        let night = self.is_night();
        let (ink, back, faint) = if night {
            // 夜晚要比白天**更暗**，不是反白：白天的底色本來就是深色主題，
            // 反過來會變成亮底（使用者說 700 分之後太亮）。近黑的底、灰色的
            // 剪影 —— 恐龍、仙人掌、翼龍、地面一起暗下去。
            let bg = theme.c(Color::Rgb(6, 8, 14));
            let fg = theme.c(theme.palette.dim);
            match theme.depth_is_monochrome() {
                // 單色終端沒有顏色可翻，用反白
                true => (
                    Style::default().add_modifier(Modifier::REVERSED),
                    Style::default().add_modifier(Modifier::REVERSED),
                    Style::default()
                        .add_modifier(Modifier::REVERSED)
                        .add_modifier(Modifier::DIM),
                ),
                false => (
                    Style::default().fg(fg).bg(bg),
                    Style::default().bg(bg),
                    Style::default().fg(fg).bg(bg).add_modifier(Modifier::DIM),
                ),
            }
        } else {
            // 白天的底色也要明確給：`Style::default()` 是「終端機的預設底色」，
            // 白底的終端機上淺灰的恐龍就消失了（死掉那格用的是 faint 深灰，
            // 反而看得見 —— 使用者回報「活著時大曝光、死了才是黑的」）。
            (
                theme.style(theme.palette.fg),
                theme.surface_style(),
                theme.faint_style(),
            )
        };

        let score = format!("HI {:05}   {:05}", self.best(), self.score());
        let sw = format::width(&score) as u16;
        let field = Rect {
            y: area.y + 1,
            height: area.height - 1,
            ..area
        };
        // 整塊場地塗底色：夜晚是反過來的底色，白天是視窗的底色 ——
        // 兩種都不能留給終端機決定。
        buf.set_style(field, back);
        if sw < area.width {
            buf.set_string(area.x + area.width - sw, area.y, score, theme.dim_style());
        }

        let cols = field.width as i32;
        let rows = field.height as i32;
        let sw_px = cols * SX;
        let sh_px = rows * SY;
        self.w.set((sw_px - 4) as f32);
        let ground_y = sh_px - 1;

        let mut grid = vec![0u8; (cols * rows) as usize];
        let put = |grid: &mut Vec<u8>, px: i32, py: i32| {
            if px < 0 || py < 0 || px >= sw_px || py >= sh_px {
                return;
            }
            let (cx, cy) = (px / SX, py / SY);
            grid[(cy * cols + cx) as usize] |= DOTS[(px % SX) as usize][(py % SY) as usize];
        };
        let blit = |grid: &mut Vec<u8>, art: &[&str], x: i32, bottom: i32| {
            for (i, row) in art.iter().enumerate() {
                let py = bottom - (art.len() as i32 - 1 - i as i32);
                for (j, ch) in row.chars().enumerate() {
                    if ch != '.' {
                        put(grid, x + j as i32, py);
                    }
                }
            }
        };

        // 地面：一條連續的線
        for px in 0..sw_px {
            put(&mut grid, px, ground_y);
        }

        let t = now.duration_since(self.started).as_secs_f32();
        for o in &self.obstacles {
            let art = o.kind.art(t);
            let unit = o.kind.unit_w();
            for i in 0..o.size {
                blit(
                    &mut grid,
                    art,
                    o.x.round() as i32 + i * unit,
                    ground_y - 1 - o.kind.base(),
                );
            }
        }

        let art: &[&str] = if self.is_over() {
            &CRASH
        } else if self.ducking(now) {
            &DUCK
        } else if self.y > 0.0 || (t * 10.0) as i64 % 2 == 0 {
            // 空中腿不動 —— 跳起來還在跑步是最容易看出貼圖感的地方
            &RUN_A
        } else {
            &RUN_B
        };
        let dino_x = if self.ducking(now) {
            Self::duck_x()
        } else {
            DINO_X
        };
        blit(&mut grid, art, dino_x, ground_y - 1 - self.y.round() as i32);

        for cy in 0..rows {
            for cx in 0..cols {
                let bits = grid[(cy * cols + cx) as usize];
                if bits == 0 {
                    continue;
                }
                if let Some(c) = buf.cell_mut((field.x + cx as u16, field.y + cy as u16)) {
                    c.set_symbol(&braille_char(bits).to_string());
                    c.set_style(if self.is_over() { faint } else { ink });
                }
            }
        }

        if self.is_over() {
            let msg = "G A M E   O V E R";
            let x = field.x + field.width.saturating_sub(format::width(msg) as u16) / 2;
            let y = field.y + 1;
            buf.set_string(x, y, msg, theme.bold(theme.palette.accent));
            if let Some(p) = panel {
                Self::render_board(buf, field, theme, p);
            }
        }
    }

    /// 榜 + 名字輸入框：浮在場地上半部的一塊，有自己的底色，蓋在雲和
    /// 仙人掌上面。恐龍與地面留在下面 —— 那是「你死在這裡」的畫面。
    fn render_board(buf: &mut Buffer, field: Rect, theme: &Theme, p: &OverPanel<'_>) {
        use crate::collectors::util::username;
        use crate::scoreboard::{NAME_WIDTH, TOP};
        // 標題 + 五名 + 空一列 + 輸入框/訊息，上下各留一列
        let rows = 1 + TOP + 1 + 1;
        let w: u16 = 50.min(field.width);
        let h = ((rows + 2) as u16).min(field.height.saturating_sub(4));
        if w < 32 || h < 6 {
            return;
        }
        let bx = Rect {
            x: field.x + (field.width - w) / 2,
            y: field.y + 3,
            width: w,
            height: h,
        };
        let blank = " ".repeat(w as usize);
        for y in bx.y..bx.y + bx.height {
            buf.set_string(bx.x, y, &blank, theme.surface_style());
        }
        let ix = bx.x + 2;
        let iw = (w - 4) as usize;
        let mut y = bx.y + 1;
        buf.set_string(ix, y, "TOP 5", theme.bold(theme.palette.accent));
        buf.set_string(
            ix + 5,
            y,
            if p.shared {
                " · 這台機器所有人"
            } else {
                " · 只有你（共用榜未啟用）"
            },
            theme.dim_style(),
        );
        y += 1;
        if p.top.is_empty() {
            buf.set_string(ix, y, "還沒有人上榜 —— 這一局就是第一筆", theme.dim_style());
        }
        for (i, e) in p.top.iter().enumerate().take(TOP) {
            if y + 2 >= bx.y + bx.height {
                break;
            }
            // 名字 → 分數 → 來自哪個帳號（檔的 owner，改不了）。
            // 帳號跟名字一樣就不重複寫。
            let account = username(e.uid);
            let who = if account == e.name {
                String::new()
            } else {
                format!("  @{account}")
            };
            let line = format!(
                "{:>2}. {}  {:>5}{who}",
                i + 1,
                format::pad(&e.name, NAME_WIDTH),
                e.score
            );
            buf.set_string(
                ix,
                y,
                format::truncate(&line, iw),
                theme.style(theme.palette.fg),
            );
            y += 1;
        }
        let last = bx.y + bx.height - 2;
        if let Some(t) = p.typed {
            let label = "上榜了！名字：";
            buf.set_string(ix, last, label, theme.bold(theme.palette.warning));
            let x = ix + format::width(label) as u16;
            let room = iw.saturating_sub(format::width(label) + 1);
            buf.set_string(
                x,
                last,
                format!("{}▏", format::truncate(t, room)),
                theme.style(theme.palette.fg),
            );
        } else if let Some(n) = p.note {
            buf.set_string(ix, last, format::truncate(n, iw), theme.dim_style());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorDepth;

    /// 固定種子的一局：同樣的種子永遠得到同樣的障礙物序列。
    fn game(seed: u64) -> (Dino, Instant) {
        let now = Instant::now();
        let mut g = Dino::with_rng(now, 0, Rng::from_seed(seed));
        // 跟真正的場地一樣寬。窄了障礙物一生出來就在眼前，
        // 測的就不是「關卡有沒有解」而是「場地夠不夠寬」。
        g.w.set((FIELD_W as i32 * SX - 4) as f32);
        // 跳過開場的無障礙時間
        g.started = now - Duration::from_millis(3100);
        (g, now)
    }
    /// 推進 `secs` 秒，每 `FRAME` 一步。
    fn run(g: &mut Dino, t: &mut Instant, secs: f32) {
        let steps = (secs / FRAME.as_secs_f32()).round() as usize;
        for _ in 0..steps {
            *t += FRAME;
            g.tick(*t);
        }
    }

    #[test]
    fn the_jump_matches_chromiums_airtime_and_height() {
        // Chromium 自己的演算法跑出來是滯空 0.583 秒、最高 1.94 個恐龍高。
        // 這個測試把換算後的結果釘住 —— 之後有人調 renderer 不小心動到
        // 物理常數，這裡會擋下來。
        let (mut g, mut t) = game(1);
        g.started = t; // 開場三秒沒有障礙物，量跳躍時不會被撞死打斷
        g.on_key(Key::Char(' '), t);
        let mut apex: f32 = 0.0;
        let start = t;
        let mut landed = None;
        for _ in 0..200 {
            t += Duration::from_millis(5);
            g.tick(t);
            apex = apex.max(g.y);
            if g.y <= 0.0 && landed.is_none() && t > start + Duration::from_millis(50) {
                landed = Some(t);
            }
        }
        let air = landed
            .expect("沒有落地")
            .duration_since(start)
            .as_secs_f32();
        let dino_h = RUN_A.len() as f32;
        assert!(
            (0.50..=0.66).contains(&air),
            "滯空 {air:.3}s，Chromium 是 0.583s"
        );
        assert!(
            (1.6..=2.3).contains(&(apex / dino_h)),
            "最高點 {:.2} 個恐龍高，Chromium 是 1.94",
            apex / dino_h
        );
    }

    #[test]
    fn pressing_jump_moves_the_dino_within_the_same_frame() {
        // 「按下去要立刻看得到」。舊版是「等下一個 50ms 的 tick 才改 y」，
        // 於是最壞情況按鍵後一整格都沒反應。
        let (mut g, t) = game(2);
        assert_eq!(g.y, 0.0);
        assert!(g.on_key(Key::Char(' '), t), "跳躍鍵沒有回報狀態改變");
        assert!(g.y > 0.0, "按了跳但 y 還是 0，要等下一格才動");
    }

    #[test]
    fn ducking_lasts_long_enough_to_be_seen_and_to_dodge() {
        // 終端機收不到放開鍵，所以蹲下必須自己撐一段時間。
        let (mut g, mut t) = game(3);
        g.started = t; // 開場無障礙，專心量蹲
        g.on_key(Key::Down, t);
        assert!(g.ducking(t), "按了 ↓ 卻沒有蹲");
        run(&mut g, &mut t, 1.9);
        assert!(g.ducking(t), "不到兩秒就站起來：蓋不過起始延遲或管線卡頓");
        run(&mut g, &mut t, 0.3);
        assert!(!g.ducking(t), "蹲著不起來");
    }

    #[test]
    fn holding_down_never_stands_up_even_when_the_input_pipeline_stalls() {
        // 按住 ↓：先等作業系統的起始延遲，然後每 30 ms 一個；中途管線
        // 卡 900 ms（事件晚到、之後一次補送）。整段任何一格都不能站起來 ——
        // 站起來的那一格正好在翼龍底下就死了（使用者回報「有時候閃一幀」）。
        for delay in [375u64, 660, 1000, 1800] {
            let (mut g, mut t) = game(3);
            // 這一段跑 4.5 秒，比開場的無障礙時間長：把開場往後推，不然
            // 第一根仙人掌會在 4.3 秒撞死牠，測的就變成「有沒有跳」。
            g.started = t + Duration::from_secs(10);
            g.on_key(Key::Down, t);
            let start = t;
            let mut next = start + Duration::from_millis(delay);
            let stall = (
                start + Duration::from_millis(2500),
                start + Duration::from_millis(3400),
            );
            while t < start + Duration::from_millis(4500) {
                t += FRAME;
                g.tick(t);
                let stalled = t >= stall.0 && t < stall.1;
                if t >= next && !stalled {
                    g.on_key(Key::Down, t);
                    next += Duration::from_millis(30);
                }
                assert!(
                    g.ducking(t),
                    "起始延遲 {delay} ms：按住不放卻在 {:.0} ms 站起來了",
                    t.duration_since(start).as_secs_f32() * 1000.0
                );
            }
        }
    }

    #[test]
    fn releasing_the_key_stands_up_within_the_hold() {
        let (mut g, mut t) = game(3);
        g.started = t;
        for _ in 0..20 {
            g.on_key(Key::Down, t);
            run(&mut g, &mut t, 0.05);
        }
        run(&mut g, &mut t, 1.9);
        assert!(g.ducking(t), "放開不到兩秒就站起來");
        run(&mut g, &mut t, 0.2);
        assert!(!g.ducking(t), "放開兩秒後還蹲著");
    }

    #[test]
    fn the_hitbox_stays_low_a_little_longer_than_the_sprite() {
        // 畫面站起來之後一秒內，碰撞箱還算蹲：極端的管線卡頓會讓畫面閃一下，
        // 但不該因此死掉。
        let (mut g, mut t) = game(3);
        g.started = t;
        g.on_key(Key::Down, t);
        let (.., duck_top) = g.hitbox(t);
        run(&mut g, &mut t, 2.1);
        assert!(!g.ducking(t), "畫面應該站起來了");
        let (.., top) = g.hitbox(t);
        assert!(
            (top - duck_top).abs() < 0.01,
            "剛站起來碰撞箱就變高了：{top} vs {duck_top}"
        );
        run(&mut g, &mut t, 1.0);
        let (.., top) = g.hitbox(t);
        assert!(top > duck_top + 5.0, "一秒之後碰撞箱該回到站著的高度");
        // 跳躍立刻取消保留：空中的碰撞箱是站著的
        g.on_key(Key::Down, t);
        g.on_key(Key::Char(' '), t);
        run(&mut g, &mut t, 0.05);
        assert!(g.y > 0.0);
        let (.., top) = g.hitbox(t);
        assert!(top > duck_top + 5.0, "跳起來了碰撞箱還算蹲");
    }

    #[test]
    fn ducking_never_reaches_further_forward_than_standing() {
        // 蹲下是為了躲，不是為了把頭伸到仙人掌上 ——
        // 前緣不能比站著的時候更靠前，否則蹲下反而更容易撞到。
        let (mut g, t) = game(21);
        let (_, stand_front, ..) = g.hitbox(t);
        g.on_key(Key::Down, t);
        assert!(g.ducking(t));
        let (duck_back, duck_front, _, duck_top) = g.hitbox(t);
        assert!(
            duck_front <= stand_front + 0.5,
            "蹲下的前緣 {duck_front} 比站著的 {stand_front} 更靠前"
        );
        assert!(duck_back >= 0.0, "蹲下的剪影伸出畫面左邊了");
        assert!(duck_top < RUN_A.len() as f32 - 2.0, "蹲下沒有比站著矮");
    }

    #[test]
    fn down_in_the_air_drops_faster_than_falling() {
        let (mut g, mut t) = game(4);
        g.on_key(Key::Char(' '), t);
        run(&mut g, &mut t, 0.25);
        let high = g.y;
        let mut fast = Dino::with_rng(t, 0, Rng::from_seed(4));
        fast.w.set(140.0);
        fast.y = high;
        fast.vy = g.vy;
        fast.started = g.started;
        fast.last = t;
        let mut slow = fast.clone_for_test();
        fast.on_key(Key::Down, t);
        let mut t2 = t;
        for _ in 0..4 {
            t2 += FRAME;
            fast.tick(t2);
            slow.tick(t2);
        }
        assert!(
            fast.y < slow.y,
            "空中按 ↓ 沒有掉得比較快（{:.1} vs {:.1}）",
            fast.y,
            slow.y
        );
    }

    #[test]
    fn a_dead_dino_only_restarts_on_the_keys_the_hint_promises() {
        let (mut g, t) = game(5);
        g.over_at = Some(t);
        for key in [
            Key::Char('t'),
            Key::Char('m'),
            Key::Left,
            Key::Char('1'),
            Key::Down,
        ] {
            g.on_key(key, t);
            assert!(g.is_over(), "{key:?} 不該重來");
        }
        // 剛死的那一瞬間，跳躍鍵也不算 —— 否則死掉當下還按著就直接重開
        g.on_key(Key::Char(' '), t);
        assert!(g.is_over(), "死掉當下按跳就重來了");
        assert!(g.on_key(Key::Char(' '), t + Duration::from_millis(800)));
        assert!(!g.is_over(), "過了緩衝時間跳躍鍵仍然不能重來");

        let (mut g, t) = game(6);
        g.over_at = Some(t);
        assert!(g.on_key(Key::Char('r'), t), "r 沒有重來");
        assert!(!g.is_over());
    }

    #[test]
    fn the_same_seed_replays_the_same_obstacles() {
        // 直接驅動生成器，不靠「跑一段時間」—— 撞死之後就不生了，
        // 那樣只會比較到前兩個。
        let seq = |seed| {
            let (mut g, t) = game(seed);
            g.speed = 10.0; // 讓翼龍與群組都在可抽範圍內
            for _ in 0..60 {
                g.obstacles.clear();
                g.spawn(t);
            }
            g.spawn_log.clone()
        };
        let a = seq(42);
        assert_eq!(a.len(), 60);
        assert_eq!(a, seq(42), "同一個種子跑出不一樣的序列");
        assert_ne!(a, seq(4242), "不同種子跑出一模一樣的序列");
        // 三種障礙物都要抽得到
        for family in 0..3u8 {
            assert!(
                a.iter().any(|(f, _)| *f == family),
                "60 次都沒抽到第 {family} 種障礙物"
            );
        }
        assert!(a.iter().any(|(_, n)| *n > 1), "從來沒有成群的仙人掌");
    }

    #[test]
    fn production_does_not_use_a_fixed_seed() {
        // 正式遊玩每一局都要不一樣。用時間當種子是不行的 ——
        // 同一秒開兩局會拿到同一串。
        let a = Rng::from_entropy();
        let b = Rng::from_entropy();
        assert_ne!(a.0, b.0, "兩次 from_entropy 拿到同一個種子");
    }

    #[test]
    fn pterodactyls_unlock_at_chromiums_speed_not_at_a_hardcoded_score() {
        // Chrome 的條件是「速度 ≥ 8.5」，不是 `score >= 500`。
        let (mut g, _t) = game(7);
        assert!(g.speed < 8.5);
        g.history.clear();
        // 速度不夠時抽不到翼龍
        for _ in 0..200 {
            g.obstacles.clear();
            g.spawn(Instant::now());
            assert!(
                !matches!(g.obstacles[0].kind, Kind::Pterodactyl(_)),
                "速度只有 {} 就出現翼龍了",
                g.speed
            );
        }
        // 加速到解鎖之後就抽得到
        g.speed = 9.0;
        let mut saw = false;
        for _ in 0..400 {
            g.obstacles.clear();
            g.history.clear();
            g.spawn(Instant::now());
            if matches!(g.obstacles[0].kind, Kind::Pterodactyl(_)) {
                saw = true;
                break;
            }
        }
        assert!(saw, "速度到了 9.0 還是抽不到翼龍");
    }

    #[test]
    fn night_follows_chromiums_seven_hundred_point_inversion() {
        let (mut g, _t) = game(8);
        for (score, want) in [
            (0u32, false),
            (699, false),
            (700, true),
            (1399, true),
            (1400, false),
        ] {
            g.distance = score as f32 / chrome::DISTANCE_COEFFICIENT;
            assert_eq!(g.is_night(), want, "分數 {score} 的日夜不對");
        }
    }

    #[test]
    fn every_level_the_generator_makes_is_actually_solvable() {
        // 「跳得過去嗎」不靠感覺，也不靠一套手寫的策略 —— 手寫策略跳不過
        // 只證明策略笨（三連大仙人掌就是：看到就跳會落在最後一棵上，
        // 晚三格再跳就過了）。這裡對每個動作找**最早哪一格做能活**：
        // 現在做能活就做，等幾格才能活就等，怎麼做都不能活才是生成器
        // 真的做出了無解的關卡。
        let frames = |secs: f32| (secs / FRAME.as_secs_f32()).round() as usize;
        let trigger = frames(0.32); // 「再不管就撞到」的視野
        let horizon = frames(0.70); // 一個動作救不救得了眼前這一個
        let max_wait = frames(0.40); // 最多等這麼久再動
        let survives = |g: &Dino, t: Instant, wait: usize, act: u8, horizon: usize| -> bool {
            let mut sim = g.clone_for_test();
            let mut tt = t;
            for _ in 0..wait {
                tt += FRAME;
                sim.tick(tt);
                if sim.is_over() {
                    return false;
                }
            }
            match act {
                1 => {
                    sim.on_key(Key::Char(' '), tt);
                }
                2 => {
                    sim.on_key(Key::Down, tt);
                }
                _ => {}
            }
            for _ in 0..horizon {
                tt += FRAME;
                sim.tick(tt);
                if sim.is_over() {
                    return false;
                }
            }
            true
        };
        for seed in [1u64, 7, 99, 12345, 65535] {
            let (mut g, mut t) = game(seed);
            let mut saw = [false; 3];
            let mut acted = 0;
            for _ in 0..2500 {
                for o in &g.obstacles {
                    saw[o.kind.family() as usize] = true;
                }
                // 在空中就什麼都不能做 —— 那不是「無解」，只是這一格沒有選擇
                if g.y <= 0.0 && !survives(&g, t, 0, 0, trigger) {
                    // 最早哪一格跳 / 蹲能活？
                    let plan = |act: u8| (0..=max_wait).find(|w| survives(&g, t, *w, act, horizon));
                    match (plan(1), plan(2)) {
                        (Some(0), _) => {
                            g.on_key(Key::Char(' '), t);
                            acted += 1;
                        }
                        (_, Some(0)) => {
                            g.on_key(Key::Down, t);
                            acted += 1;
                        }
                        (Some(_), _) | (_, Some(_)) => {} // 等，下一格再看
                        (None, None) => panic!(
                            "種子 {seed}：{} 分（速度 {:.2}）恐龍 y={:.1} 跳不過也蹲不掉：{}",
                            g.score(),
                            g.speed,
                            g.y,
                            g.obstacles
                                .iter()
                                .map(|o| format!(
                                    "{:?}×{} x{:.0}..{:.0} y{}..{}",
                                    o.kind,
                                    o.size,
                                    o.x,
                                    o.x + o.width() as f32,
                                    o.kind.base(),
                                    o.kind.base() + o.kind.height()
                                ))
                                .collect::<Vec<_>>()
                                .join(" | ")
                        ),
                    }
                }
                t += FRAME;
                g.tick(t);
                assert!(!g.is_over(), "種子 {seed}：{} 分還是撞死了", g.score());
            }
            assert!(g.score() > 200, "種子 {seed} 只跑了 {} 分", g.score());
            assert!(acted > 20, "種子 {seed} 幾乎不用動就過關了（{acted} 次）");
            assert!(saw[0] && saw[1], "種子 {seed} 沒有出現兩種仙人掌");
        }
    }

    #[test]
    fn obstacles_are_never_closer_than_a_jump() {
        // 隨機不能生出物理上過不去的序列
        let (mut g, mut t) = game(31337);
        for _ in 0..4000 {
            t += FRAME;
            g.tick(t);
            let mut xs: Vec<(f32, f32)> = g
                .obstacles
                .iter()
                .map(|o| (o.x, o.x + o.width() as f32))
                .collect();
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            for pair in xs.windows(2) {
                let gap = pair[1].0 - pair[0].1;
                assert!(
                    gap > 8.0,
                    "兩個障礙物只隔 {gap:.1} 子像素，中間來不及落地再起跳"
                );
            }
        }
    }

    #[test]
    fn it_survives_any_terminal_size_including_absurd_ones() {
        let theme = Theme::new("default", ColorDepth::TrueColor);
        for (w, h) in [(1u16, 1u16), (10, 5), (40, 8), (80, 24), (300, 90)] {
            let (mut g, mut t) = game(11);
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for _ in 0..80 {
                t += FRAME;
                g.tick(t);
                g.render(&mut buf, area, &theme, t, None);
            }
        }
    }

    #[test]
    fn nothing_is_drawn_outside_the_area() {
        let theme = Theme::new("default", ColorDepth::TrueColor);
        let full = Rect::new(0, 0, 100, 30);
        let area = Rect::new(10, 5, 70, 14);
        let (mut g, mut t) = game(12);
        let mut buf = Buffer::empty(full);
        for _ in 0..120 {
            t += FRAME;
            g.tick(t);
            g.render(&mut buf, area, &theme, t, None);
        }
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
                        "畫到了遊戲區外的 ({x},{y})"
                    );
                }
            }
        }
    }

    #[test]
    fn the_field_is_wide_and_short_like_the_real_thing() {
        const { assert!(FIELD_W >= FIELD_H * 4, "場地不夠寬") };
        let dino_h = RUN_A.len() as u16;
        assert!(
            dino_h * 2 < FIELD_H * 4,
            "恐龍佔了畫面高度的 {dino_h}/{}，太大了",
            FIELD_H * 4
        );
    }

    #[test]
    fn the_pixel_scale_matches_the_sprites() {
        // 兩個換算常數必須真的等於「我們的剪影 ÷ Chrome 的剪影」，
        // 不然所有由 Chrome 單位換算過來的長度都會偏掉。
        assert!((PX_X - RUN_A[0].len() as f32 / chrome::TREX_W).abs() < 1e-6);
        assert!((PX_Y - RUN_A.len() as f32 / chrome::TREX_H).abs() < 1e-6);
    }

    #[test]
    fn the_proportions_follow_chromiums_sprites() {
        // 換算是否正確：仙人掌相對恐龍的高度必須跟原版一樣
        let dino = RUN_A.len() as f32;
        assert!(
            ((CACTUS_S.len() as f32 / dino) - (35.0 / 47.0)).abs() < 0.1,
            "小仙人掌的相對高度跑掉了"
        );
        assert!(
            ((CACTUS_L.len() as f32 / dino) - (50.0 / 47.0)).abs() < 0.1,
            "大仙人掌的相對高度跑掉了"
        );
        assert!(
            (DUCK.len() as f32 / dino - 25.0 / 47.0).abs() < 0.1,
            "蹲下的高度跑掉了"
        );
    }
}
