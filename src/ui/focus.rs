//! 空間式、兩層的焦點模型。
//!
//! # 導航模型（像螢幕的 OSD 選單，但層數不設限）
//!
//! 焦點是一棵樹上的一個節點：
//!
//! ```text
//!  分頁層（沒有焦點）  ← →  切換分頁      ↓ / Enter  進入這一頁
//!         ↑ Esc                                  ↓
//!  第 0 層（面板）     ↑↓←→ 在兄弟之間移動   Enter  往下鑽
//!         ↑ Esc                                  ↓
//!  第 1 層（面板裡的列）  ↑↓←→ 在兄弟之間移動  Enter  再往下鑽
//!         ↑ Esc                                  ↓
//!  第 2 層、第 3 層…    只要頁面有登記，就能一直往下
//! ```
//!
//! **層數不設限**。方向鍵永遠只在同一層的兄弟之間移動，所以不管樹多深，
//! 「上下左右」的意思都不會變。要更深的資訊就 Enter，要退出來就 Esc。
//!
//! # 為什麼不能只到面板層
//!
//! 只到面板層的話，「Logical CPUs」「Avg Frequency」「Core clock」這些
//! 實際的數字就選不到，說明反而比寫死的清單還粗。往下鑽一層之後，
//! 畫面上看得到的每一列都能選、都能解釋；再往下還能選到單一行程，
//! 未來的「停掉這個行程」之類的操作就掛在那一層。

use std::cell::RefCell;

use ratatui::layout::Rect;

use crate::metrics::describe::{DescribeTarget, EntityRef};

/// 畫面上一塊可以被選取、可以被解釋的區域。
#[derive(Debug, Clone, PartialEq)]
pub struct FocusRegion {
    pub rect: Rect,
    /// 按 `e` 時要解釋什麼。可能是固定 metric，也可能是這台機器上的某個具體物件。
    pub target: DescribeTarget,
    /// 畫面上實際顯示的名稱（不是另外編的名詞）
    pub label: String,
    /// `None` 代表這是一個面板；`Some(i)` 代表它是第 i 個面板裡的一列。
    pub parent: Option<usize>,
    /// 這一塊是不是一份可以捲動的清單（行程表、使用者清單）。
    /// 是的話，進到 Item 層時 ↑↓ 用來選列，而不是在面板之間跳。
    pub scrollable: bool,
    /// 這一列在**整份清單**裡的絕對索引（只有清單列才有）。
    ///
    /// 只有看得到的列會被登記，所以「在兄弟之間排第幾」不等於
    /// 「在清單裡排第幾」—— 清單一捲動就差一個視窗偏移量。
    /// 選取狀態必須跟著絕對索引走，不然選到一半會跳回清單開頭。
    pub list_index: Option<usize>,
}

/// 每次繪製時重建的可選區域清單。
#[derive(Debug, Default)]
pub struct FocusRegistry {
    regions: RefCell<Vec<FocusRegion>>,
}

impl FocusRegistry {
    pub fn clear(&self) {
        self.regions.borrow_mut().clear();
    }

    /// 登記一個面板，回傳它的索引（登記面板裡的列時要用）。
    pub fn add_panel(
        &self,
        rect: Rect,
        metric_id: &'static str,
        label: impl Into<String>,
    ) -> usize {
        self.push(FocusRegion {
            rect,
            target: DescribeTarget::Metric(metric_id),
            label: label.into(),
            parent: None,
            scrollable: false,
            list_index: None,
        })
    }

    /// 登記一個代表**具體系統物件**的面板（GPU 裝置、網路介面、磁碟…）。
    pub fn add_entity_panel(
        &self,
        rect: Rect,
        entity: EntityRef,
        label: impl Into<String>,
    ) -> usize {
        self.push(FocusRegion {
            rect,
            target: DescribeTarget::Entity(entity),
            label: label.into(),
            parent: None,
            scrollable: false,
            list_index: None,
        })
    }

    /// 登記一個代表具體系統物件的子節點（單一行程、單一掛載點…）。
    pub fn add_entity_item(
        &self,
        parent: usize,
        rect: Rect,
        entity: EntityRef,
        label: impl Into<String>,
    ) -> usize {
        self.push(FocusRegion {
            rect,
            target: DescribeTarget::Entity(entity),
            label: label.into(),
            parent: Some(parent),
            scrollable: false,
            list_index: None,
        })
    }

    /// 登記一個「內容是可捲動清單」的面板。
    pub fn add_list_panel(
        &self,
        rect: Rect,
        metric_id: &'static str,
        label: impl Into<String>,
    ) -> usize {
        self.push(FocusRegion {
            rect,
            target: DescribeTarget::Metric(metric_id),
            label: label.into(),
            parent: None,
            scrollable: true,
            list_index: None,
        })
    }

    /// 登記任意一層的子節點。`parent` 是上一層回傳的索引。
    ///
    /// 想做幾層就傳幾層 —— 這裡不限制深度。
    pub fn add_child(
        &self,
        parent: usize,
        rect: Rect,
        metric_id: &'static str,
        label: impl Into<String>,
    ) -> usize {
        self.push(FocusRegion {
            rect,
            target: DescribeTarget::Metric(metric_id),
            label: label.into(),
            parent: Some(parent),
            scrollable: false,
            list_index: None,
        })
    }

    /// 登記一個「內容還可以再往下鑽」的子節點。
    pub fn add_list_child(
        &self,
        parent: usize,
        rect: Rect,
        metric_id: &'static str,
        label: impl Into<String>,
    ) -> usize {
        self.push(FocusRegion {
            rect,
            target: DescribeTarget::Metric(metric_id),
            label: label.into(),
            parent: Some(parent),
            scrollable: true,
            list_index: None,
        })
    }

    /// 登記可捲動清單裡的一列，並記住它在整份清單裡的絕對索引。
    ///
    /// 用 `add_item` 登記清單列的話，選取狀態只能靠「排第幾個兄弟」回推，
    /// 那個數字在清單捲動之後就不對了。有絕對索引才對得回原本那一列。
    pub fn add_row(
        &self,
        parent: usize,
        rect: Rect,
        metric_id: &'static str,
        label: impl Into<String>,
        index: usize,
    ) -> usize {
        self.push(FocusRegion {
            rect,
            target: DescribeTarget::Metric(metric_id),
            label: label.into(),
            parent: Some(parent),
            scrollable: false,
            list_index: Some(index),
        })
    }

    /// 登記面板裡的一列（`add_child` 的別名，讀起來比較直觀）。
    pub fn add_item(
        &self,
        parent: usize,
        rect: Rect,
        metric_id: &'static str,
        label: impl Into<String>,
    ) -> usize {
        self.add_child(parent, rect, metric_id, label)
    }

    /// 找出 `parent` 底下絕對索引等於 `index` 的那一列。
    /// 畫面重畫之後焦點要重新對回選取的那一列，靠的就是這個。
    pub fn row_with_index(&self, parent: usize, index: usize) -> Option<usize> {
        self.regions
            .borrow()
            .iter()
            .position(|r| r.parent == Some(parent) && r.list_index == Some(index))
    }

    fn push(&self, r: FocusRegion) -> usize {
        if r.rect.width == 0 || r.rect.height == 0 {
            // 零面積的區域選不到也框不出來，但仍要回傳一個索引讓呼叫端能繼續
            return usize::MAX;
        }
        let mut v = self.regions.borrow_mut();
        v.push(r);
        v.len() - 1
    }

    pub fn len(&self) -> usize {
        self.regions.borrow().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn get(&self, i: usize) -> Option<FocusRegion> {
        self.regions.borrow().get(i).cloned()
    }
    pub fn all(&self) -> Vec<FocusRegion> {
        self.regions.borrow().clone()
    }

    /// 全部面板的索引。
    pub fn panels(&self) -> Vec<usize> {
        self.regions
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, r)| r.parent.is_none())
            .map(|(i, _)| i)
            .collect()
    }

    /// 某個面板底下所有列的索引。
    pub fn items_of(&self, panel: usize) -> Vec<usize> {
        self.regions
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, r)| r.parent == Some(panel))
            .map(|(i, _)| i)
            .collect()
    }

    /// 這個索引所屬的面板（自己是面板就回自己）。
    pub fn panel_of(&self, i: usize) -> Option<usize> {
        let v = self.regions.borrow();
        let r = v.get(i)?;
        Some(r.parent.unwrap_or(i))
    }

    // ── 通用的樹操作（層數不設限）────────────────────────────────────

    /// 最上層的節點（沒有 parent 的）。
    pub fn roots(&self) -> Vec<usize> {
        self.regions
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, r)| r.parent.is_none())
            .map(|(i, _)| i)
            .collect()
    }

    /// `i` 的直接子節點。
    pub fn children_of(&self, i: usize) -> Vec<usize> {
        self.regions
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, r)| r.parent == Some(i))
            .map(|(k, _)| k)
            .collect()
    }

    /// `i` 的父節點。
    pub fn parent_of(&self, i: usize) -> Option<usize> {
        self.regions.borrow().get(i).and_then(|r| r.parent)
    }

    /// 與 `i` 同一層、同一個父節點的所有節點（含自己）。
    /// 方向鍵只在這個集合裡移動。
    pub fn siblings_of(&self, i: usize) -> Vec<usize> {
        match self.parent_of(i) {
            Some(p) => self.children_of(p),
            None => self.roots(),
        }
    }

    /// `i` 在樹上的深度（最上層是 0）。
    pub fn depth_of(&self, i: usize) -> usize {
        let v = self.regions.borrow();
        let mut d = 0;
        let mut cur = i;
        // 有上限的迴圈，就算資料異常也不會無窮迴圈
        while let Some(Some(p)) = v.get(cur).map(|r| r.parent) {
            d += 1;
            cur = p;
            if d > 32 {
                break;
            }
        }
        d
    }

    /// 從最上層到 `i` 的路徑（給麵包屑用）。
    pub fn path_to(&self, i: usize) -> Vec<usize> {
        let v = self.regions.borrow();
        let mut path = vec![i];
        let mut cur = i;
        while let Some(Some(p)) = v.get(cur).map(|r| r.parent) {
            path.push(p);
            cur = p;
            if path.len() > 32 {
                break;
            }
        }
        path.reverse();
        path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// 在**指定的候選集合**裡，從 `from` 往 `dir` 找最近的一個。
///
/// 候選集合由呼叫端決定（面板層就傳全部面板，項目層就傳同一個面板裡的列），
/// 這樣方向鍵永遠只在同一層裡移動，不會突然跳到別層去。
///
/// 判準：
/// 1. **優先選在垂直軸上有重疊的** —— 往下就找同一欄的下一個。只比中心點的話，
///    一個比較高的鄰欄面板中心會落得更低，↓ 就會跳到隔壁欄。
/// 2. 有重疊的候選裡挑邊界距離最近的。
/// 3. 完全沒有重疊時才退而找該方向最近的。
pub fn nearest(
    regions: &[FocusRegion],
    candidates: &[usize],
    from: usize,
    dir: Dir,
) -> Option<usize> {
    let c = regions.get(from)?.rect;

    let (c_lo, c_hi) = match dir {
        Dir::Up | Dir::Down => (c.x as i32, c.x as i32 + c.width as i32),
        Dir::Left | Dir::Right => (c.y as i32, c.y as i32 + c.height as i32),
    };

    let mut overlapping: Option<(i64, usize)> = None;
    let mut fallback: Option<(i64, usize)> = None;

    for &i in candidates {
        if i == from {
            continue;
        }
        let Some(r) = regions.get(i) else { continue };
        let t = r.rect;
        let gap = match dir {
            Dir::Down => t.y as i32 - (c.y as i32 + c.height as i32),
            Dir::Up => c.y as i32 - (t.y as i32 + t.height as i32),
            Dir::Right => t.x as i32 - (c.x as i32 + c.width as i32),
            Dir::Left => c.x as i32 - (t.x as i32 + t.width as i32),
        };
        // 候選必須**真的**在那個方向上：它的起點要比目前這塊更遠。
        //
        // 只看間隙不夠。一列高的東西排成一排時（Admin 的分頁列），
        // 往下的間隙會算成 -1，落在原本的容忍範圍裡，於是 ↑↓ 就變成了
        // 左右切換。比起點才分得出「在下面」和「在旁邊」。
        let ahead = match dir {
            Dir::Down => t.y > c.y,
            Dir::Up => t.y < c.y,
            Dir::Right => t.x > c.x,
            Dir::Left => t.x < c.x,
        };
        if !ahead {
            continue;
        }
        let gap = gap.max(0);
        let (t_lo, t_hi) = match dir {
            Dir::Up | Dir::Down => (t.x as i32, t.x as i32 + t.width as i32),
            Dir::Left | Dir::Right => (t.y as i32, t.y as i32 + t.height as i32),
        };
        let overlap = (c_hi.min(t_hi) - c_lo.max(t_lo)).max(0);

        if overlap > 0 {
            let score = gap as i64 * 10_000 - overlap as i64;
            if overlapping.is_none_or(|(b, _)| score < b) {
                overlapping = Some((score, i));
            }
        } else {
            let perp = (c_lo - t_lo).abs() as i64;
            let score = gap as i64 * 10_000 + perp;
            if fallback.is_none_or(|(b, _)| score < b) {
                fallback = Some((score, i));
            }
        }
    }
    overlapping.or(fallback).map(|(_, i)| i)
}

/// 在候選集合裡依畫面順序繞一圈（找不到同方向的鄰居時用）。
pub fn wrap_within(candidates: &[usize], from: usize, forward: bool) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    let pos = candidates.iter().position(|&i| i == from)?;
    let n = candidates.len();
    Some(
        candidates[if forward {
            (pos + 1) % n
        } else {
            (pos + n - 1) % n
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(x: u16, y: u16, w: u16, h: u16, id: &'static str) -> FocusRegion {
        FocusRegion {
            rect: Rect::new(x, y, w, h),
            target: DescribeTarget::Metric(id),
            label: id.to_owned(),
            parent: None,
            scrollable: false,
            list_index: None,
        }
    }
    fn item(p: usize, x: u16, y: u16, w: u16, h: u16, id: &'static str) -> FocusRegion {
        FocusRegion {
            rect: Rect::new(x, y, w, h),
            target: DescribeTarget::Metric(id),
            label: id.to_owned(),
            parent: Some(p),
            scrollable: false,
            list_index: None,
        }
    }

    /// 總覽的實際排法：左欄三塊、右欄三塊，高度不一致
    fn overview() -> (Vec<FocusRegion>, Vec<usize>) {
        let v = vec![
            panel(0, 2, 75, 14, "cpu"),
            panel(75, 2, 75, 9, "mem"),
            panel(0, 16, 75, 11, "gpu"),
            panel(75, 11, 75, 11, "disk"),
            panel(0, 27, 75, 11, "net"),
            panel(75, 22, 75, 16, "proc"),
        ];
        let c = (0..v.len()).collect();
        (v, c)
    }

    #[test]
    fn vertical_moves_stay_in_the_same_column() {
        let (v, c) = overview();
        assert_eq!(
            nearest(&v, &c, 1, Dir::Down).map(|i| v[i].label.as_str()),
            Some("disk"),
            "Memory 往下應留在右欄"
        );
        assert_eq!(
            nearest(&v, &c, 0, Dir::Down).map(|i| v[i].label.as_str()),
            Some("gpu"),
            "CPU 往下應留在左欄"
        );
        assert_eq!(
            nearest(&v, &c, 3, Dir::Down).map(|i| v[i].label.as_str()),
            Some("proc")
        );
    }

    #[test]
    fn horizontal_moves_reach_every_panel_in_the_other_column() {
        // 使用者回報：一直按 → 永遠跳過某一格。
        // 從左欄每一塊往右，都要能到右欄對應的那一塊。
        let (v, c) = overview();
        assert_eq!(
            nearest(&v, &c, 0, Dir::Right).map(|i| v[i].label.as_str()),
            Some("mem")
        );
        assert_eq!(
            nearest(&v, &c, 2, Dir::Right).map(|i| v[i].label.as_str()),
            Some("disk")
        );
        assert_eq!(
            nearest(&v, &c, 4, Dir::Right).map(|i| v[i].label.as_str()),
            Some("proc")
        );
    }

    #[test]
    fn every_panel_is_reachable_by_repeated_moves() {
        // 「一直按 → 會永遠跳過 Core Statistics」那個 bug 的一般化檢查：
        // 從任何一塊出發，用上下左右一定要能走到全部的塊。
        let (v, c) = overview();
        for start in 0..v.len() {
            let mut seen = std::collections::HashSet::new();
            let mut frontier = vec![start];
            while let Some(cur) = frontier.pop() {
                if !seen.insert(cur) {
                    continue;
                }
                for d in [Dir::Up, Dir::Down, Dir::Left, Dir::Right] {
                    if let Some(n) = nearest(&v, &c, cur, d) {
                        frontier.push(n);
                    }
                }
            }
            assert_eq!(
                seen.len(),
                v.len(),
                "從 {} 出發只走得到 {} 塊，共有 {} 塊",
                v[start].label,
                seen.len(),
                v.len()
            );
        }
    }

    #[test]
    fn items_navigate_only_among_their_siblings() {
        // 面板裡的列上下移動時，不可以跳到別的面板去
        let v = vec![
            panel(0, 0, 40, 20, "stats"),
            item(0, 1, 1, 38, 1, "a"),
            item(0, 1, 2, 38, 1, "b"),
            item(0, 1, 3, 38, 1, "c"),
            panel(40, 0, 40, 20, "other"),
            item(4, 41, 1, 38, 1, "x"),
        ];
        let siblings = vec![1, 2, 3];
        assert_eq!(nearest(&v, &siblings, 1, Dir::Down), Some(2));
        assert_eq!(
            nearest(&v, &siblings, 3, Dir::Down),
            None,
            "最後一列往下沒有兄弟"
        );
        assert_eq!(
            nearest(&v, &siblings, 2, Dir::Right),
            None,
            "同一欄的列沒有左右鄰居"
        );
    }

    #[test]
    fn adjacent_panels_with_zero_gap_are_reachable() {
        let v = vec![panel(0, 0, 40, 10, "a"), panel(0, 10, 40, 10, "b")];
        let c = vec![0, 1];
        assert_eq!(nearest(&v, &c, 0, Dir::Down), Some(1));
        assert_eq!(nearest(&v, &c, 1, Dir::Up), Some(0));
    }

    #[test]
    fn returns_none_at_the_edges() {
        let (v, c) = overview();
        assert_eq!(nearest(&v, &c, 0, Dir::Up), None);
        assert_eq!(nearest(&v, &c, 0, Dir::Left), None);
    }

    #[test]
    fn wrap_cycles_within_the_candidate_set() {
        let c = vec![3, 7, 9];
        assert_eq!(wrap_within(&c, 9, true), Some(3));
        assert_eq!(wrap_within(&c, 3, false), Some(9));
        assert_eq!(wrap_within(&[], 0, true), None);
        assert_eq!(wrap_within(&c, 99, true), None, "不在集合裡就不該回傳東西");
    }

    #[test]
    fn tree_depth_is_not_limited_to_two_levels() {
        let reg = FocusRegistry::default();
        let a = reg.add_panel(Rect::new(0, 0, 40, 20), "proc.cpu", "Processes");
        let b = reg.add_child(a, Rect::new(1, 1, 38, 1), "proc.cpu", "pid 1322");
        let c = reg.add_child(b, Rect::new(2, 2, 36, 1), "proc.threads", "threads");
        let d = reg.add_child(c, Rect::new(3, 3, 34, 1), "proc.state", "state");
        assert_eq!(reg.depth_of(a), 0);
        assert_eq!(reg.depth_of(b), 1);
        assert_eq!(reg.depth_of(c), 2);
        assert_eq!(reg.depth_of(d), 3, "層數不該被限制在兩層");
        assert_eq!(
            reg.path_to(d),
            vec![a, b, c, d],
            "麵包屑要能一路回推到最上層"
        );
        assert_eq!(reg.parent_of(d), Some(c));
        assert_eq!(reg.children_of(b), vec![c]);
    }

    #[test]
    fn siblings_are_scoped_to_the_same_parent() {
        let reg = FocusRegistry::default();
        let p1 = reg.add_panel(Rect::new(0, 0, 40, 20), "cpu.load", "A");
        let p2 = reg.add_panel(Rect::new(40, 0, 40, 20), "mem.used", "B");
        let a1 = reg.add_child(p1, Rect::new(1, 1, 38, 1), "cpu.usage", "a1");
        let a2 = reg.add_child(p1, Rect::new(1, 2, 38, 1), "cpu.freq", "a2");
        let b1 = reg.add_child(p2, Rect::new(41, 1, 38, 1), "mem.swap", "b1");
        assert_eq!(
            reg.siblings_of(a1),
            vec![a1, a2],
            "只能在同一個父節點下移動"
        );
        assert_eq!(reg.siblings_of(b1), vec![b1]);
        assert_eq!(
            reg.siblings_of(p1),
            vec![p1, p2],
            "最上層的兄弟就是全部的根"
        );
    }

    #[test]
    fn depth_survives_malformed_data_without_hanging() {
        // 就算資料異常也不可以無窮迴圈
        let reg = FocusRegistry::default();
        let a = reg.add_panel(Rect::new(0, 0, 10, 10), "x", "x");
        assert_eq!(reg.depth_of(a), 0);
        assert_eq!(reg.depth_of(999), 0, "不存在的索引不該 panic");
    }

    #[test]
    fn registry_separates_panels_from_items() {
        let reg = FocusRegistry::default();
        let p = reg.add_panel(Rect::new(0, 0, 40, 10), "cpu.load", "Core Statistics");
        reg.add_item(p, Rect::new(1, 1, 38, 1), "cpu.usage", "Logical CPUs");
        reg.add_item(p, Rect::new(1, 2, 38, 1), "cpu.freq", "Avg Frequency");
        let p2 = reg.add_panel(Rect::new(40, 0, 40, 10), "proc.cpu", "Processes");
        assert_eq!(reg.panels(), vec![0, 3]);
        assert_eq!(reg.items_of(p), vec![1, 2]);
        assert!(reg.items_of(p2).is_empty());
        assert_eq!(reg.panel_of(1), Some(0), "列要能問出自己屬於哪個面板");
        assert_eq!(reg.panel_of(0), Some(0), "面板問自己就回自己");
    }

    #[test]
    fn zero_sized_regions_are_ignored() {
        let reg = FocusRegistry::default();
        assert_eq!(reg.add_panel(Rect::new(0, 0, 0, 5), "x", "x"), usize::MAX);
        assert!(reg.is_empty());
    }

    #[test]
    fn list_panels_are_marked_scrollable() {
        let reg = FocusRegistry::default();
        let p = reg.add_list_panel(Rect::new(0, 0, 40, 20), "proc.cpu", "Processes");
        assert!(reg.get(p).unwrap().scrollable);
        let q = reg.add_panel(Rect::new(0, 0, 40, 20), "cpu.load", "Stats");
        assert!(!reg.get(q).unwrap().scrollable);
    }
}
