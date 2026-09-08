//! 應用程式狀態機。
//!
//! 這裡只管「現在是什麼狀態、按鍵怎麼改變狀態」，不畫任何東西也不讀任何檔案。
//! 畫圖在 [`crate::ui`]，讀資料在 [`crate::collectors`]。

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::collectors::process::SortKey;
use crate::collectors::{Intervals, SystemState};
use crate::config::Config;
use crate::metrics::registry::{Focusable, PageMetrics};
use crate::privilege::protocol::{Operation, SafeSignal};
use crate::privilege::{PrivilegeClient, PrivilegeState};
use crate::theme::{ColorDepth, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    Cpu,
    Memory,
    Gpu,
    Storage,
    Network,
    Process,
    Admin,
}

/// 這些區域用 ↑↓ 捲動而不是移動焦點（行程表、管理員清單）。
const SCROLLABLE_METRICS: &[&str] = &["proc.cpu", "storage.user"];

/// 這一塊是不是用 ↑↓ 捲動（而不是移動焦點）。
pub fn is_scrollable_metric(id: &str) -> bool {
    SCROLLABLE_METRICS.contains(&id)
}

pub const VIEWS: [View; 8] = [
    View::Overview,
    View::Cpu,
    View::Memory,
    View::Gpu,
    View::Storage,
    View::Network,
    View::Process,
    View::Admin,
];

impl View {
    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Gpu => "GPU",
            Self::Storage => "Storage",
            Self::Network => "Network",
            Self::Process => "Processes",
            Self::Admin => "Admin",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            // 顯示 0 而不是反引號：旁邊是 1–6 的數字序列，
            // 放一個 ` 在那裡看起來像雜訊。反引號與 ~ 仍然是有效別名。
            Self::Overview => "0",
            Self::Cpu => "1",
            Self::Memory => "2",
            Self::Gpu => "3",
            Self::Storage => "4",
            Self::Network => "5",
            Self::Process => "6",
            Self::Admin => "A",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "overview" => Self::Overview,
            "cpu" => Self::Cpu,
            "memory" | "mem" => Self::Memory,
            "gpu" => Self::Gpu,
            "storage" | "disk" => Self::Storage,
            "network" | "net" => Self::Network,
            "process" | "proc" => Self::Process,
            "admin" => Self::Admin,
            _ => return None,
        })
    }
    /// 這一頁上有哪些可解釋的 metric。
    pub fn metrics(self) -> &'static [Focusable] {
        match self {
            Self::Cpu => PageMetrics::CPU,
            Self::Memory => PageMetrics::MEMORY,
            Self::Gpu => PageMetrics::GPU,
            Self::Storage => PageMetrics::STORAGE,
            Self::Network => PageMetrics::NETWORK,
            Self::Process => PageMetrics::PROCESS,
            // 總覽與 admin 借用 CPU 的清單當預設
            Self::Overview | Self::Admin => PageMetrics::CPU,
        }
    }
}

/// 疊在主畫面上的視窗。
#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    None,
    Help,
    /// 啟動畫面。任何鍵關掉，跟說明頁一樣。
    ///
    /// 不設自動逾時：會自己消失的畫面，使用者永遠不確定自己還有多久
    /// 可以看，反而要趕。按一下就走，主導權在人身上。
    Splash,
    /// Explain 面板：解釋目前框住的東西（固定 metric 或具體物件）。
    Explain {
        target: crate::metrics::describe::DescribeTarget,
    },
    /// 執行前的二次確認。**所有會改變系統狀態的操作都必須經過這裡。**
    Confirm(Confirmation),
    /// 顯示一段錯誤或說明。
    Message {
        title: String,
        body: String,
    },
    /// 彩蛋：恐龍跑酷。所有按鍵都屬於它。
    Dino,
}

/// 二次確認的完整內容。
///
/// 刻意把目標的 PID / 使用者 / 指令全部帶著，讓使用者在按下確認前
/// 看到的是**實際會被影響的東西**，而不是「你確定嗎？」。
#[derive(Debug, Clone, PartialEq)]
pub struct Confirmation {
    pub title: String,
    pub target_pid: i32,
    pub target_user: String,
    pub target_command: String,
    pub action: String,
    pub warning: Option<String>,
    pub operation: Operation,
    /// 使用者必須先把游標移到「確認」上，預設停在「取消」。
    pub confirmed_selected: bool,
}

impl Confirmation {
    pub fn signal(pid: i32, starttime: u64, user: &str, command: &str, signal: SafeSignal) -> Self {
        Self {
            title: format!("送出 SIG{} 給行程", signal.name()),
            target_pid: pid,
            target_user: user.to_owned(),
            target_command: command.to_owned(),
            action: format!("{} — {}", signal.name(), signal.description()),
            warning: (user == "root")
                .then(|| "這是 root 擁有的行程，中止它可能影響整台機器的服務。".to_owned()),
            operation: Operation::ProcessSignal {
                pid,
                starttime,
                signal,
            },
            confirmed_selected: false,
        }
    }
    pub fn renice(pid: i32, starttime: u64, user: &str, command: &str, nice: i8) -> Self {
        Self {
            title: "調整行程優先度".into(),
            target_pid: pid,
            target_user: user.to_owned(),
            target_command: command.to_owned(),
            action: format!("nice = {nice}（數字越大優先度越低）"),
            warning: None,
            operation: Operation::Renice {
                pid,
                starttime,
                nice,
            },
            confirmed_selected: false,
        }
    }
}

/// 非同步工作的狀態。管理員查詢可能要跑很久（掃 /home），不能卡住 UI。
#[derive(Debug, Clone, Default)]
pub enum Async<T> {
    #[default]
    Idle,
    Running {
        since: Instant,
    },
    Ready(T),
    Failed(String),
}

impl<T> Async<T> {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(v) => Some(v),
            _ => None,
        }
    }
}

/// 管理員頁面的分頁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminTab {
    Storage,
    Memory,
    Gpu,
    Network,
}

pub const ADMIN_TABS: [AdminTab; 4] = [
    AdminTab::Storage,
    AdminTab::Memory,
    AdminTab::Gpu,
    AdminTab::Network,
];

impl AdminTab {
    pub fn title(self) -> &'static str {
        match self {
            Self::Storage => "User Storage",
            Self::Memory => "User Memory",
            Self::Gpu => "GPU Users",
            Self::Network => "Sockets",
        }
    }
    /// 這個分頁主要在講哪個 metric（按 e 時解釋用）。
    pub fn metric_id(self) -> &'static str {
        match self {
            Self::Storage => "storage.user",
            Self::Memory => "proc.rss",
            Self::Gpu => "gpu.vram",
            Self::Network => "net.sockets",
        }
    }

    pub fn operation(self) -> Operation {
        match self {
            Self::Storage => Operation::StorageUsers,
            Self::Memory => Operation::UserMemory,
            Self::Gpu => Operation::GpuUsers,
            Self::Network => Operation::SocketMap,
        }
    }
}

#[derive(Default)]
pub struct AdminState {
    pub tab_index: usize,
    pub storage: Async<serde_json::Value>,
    pub memory: Async<serde_json::Value>,
    pub gpu: Async<serde_json::Value>,
    pub sockets: Async<serde_json::Value>,
    pub selected: usize,
    /// 展開某個使用者的儲存明細
    pub detail: Async<serde_json::Value>,
    pub detail_user: Option<String>,
}

impl AdminState {
    pub fn tab(&self) -> AdminTab {
        ADMIN_TABS[self.tab_index.min(ADMIN_TABS.len() - 1)]
    }
    pub fn current(&self) -> &Async<serde_json::Value> {
        match self.tab() {
            AdminTab::Storage => &self.storage,
            AdminTab::Memory => &self.memory,
            AdminTab::Gpu => &self.gpu,
            AdminTab::Network => &self.sockets,
        }
    }
    fn slot(&mut self, tab: AdminTab) -> &mut Async<serde_json::Value> {
        match tab {
            AdminTab::Storage => &mut self.storage,
            AdminTab::Memory => &mut self.memory,
            AdminTab::Gpu => &mut self.gpu,
            AdminTab::Network => &mut self.sockets,
        }
    }
}

/// 背景工作回傳的訊息。
pub enum WorkerMsg {
    Admin {
        tab: AdminTab,
        result: Result<serde_json::Value, String>,
    },
    Detail {
        user: String,
        result: Result<serde_json::Value, String>,
    },
    Mutation {
        description: String,
        result: Result<serde_json::Value, String>,
    },
}

/// 主迴圈需要 App 做的「副作用」。
///
/// 例如互動式 sudo 必須離開 alternate screen，那件事只有主迴圈能做，
/// 所以 App 只負責「提出請求」。
#[derive(Debug, Clone, PartialEq)]
pub enum SideEffect {
    None,
    /// 暫停 TUI、跑互動式 sudo、再回來。
    AuthenticateSudo,
    Quit,
}

pub struct App {
    pub state: SystemState,
    pub config: Config,
    pub theme: Theme,
    pub view: View,
    pub modal: Modal,
    pub privilege: PrivilegeClient,
    pub admin: AdminState,
    pub paused: bool,
    pub interval: Duration,
    pub sort: SortKey,
    pub filter: String,
    pub filter_editing: bool,
    pub scroll: usize,
    /// 目前框住的節點索引。`None` 代表停在「分頁層」（沒有進入頁面）。
    pub focus: Option<usize>,
    /// Explain 面板的捲動位置（內容可能比畫面長）
    pub explain_scroll: u16,
    /// 吉祥物的動畫格。**完全獨立於採樣時鐘** ——
    /// 它每秒前進幾格由 `branding.mascot_fps` 決定，而且不管它跑多快，
    /// collector 的取樣間隔都不會被動到一分一毫。
    pub anim_frame: u64,
    /// 上一次推進動畫的時間。
    pub last_anim: Instant,
    /// 系統連續閒置多久了（給 idle 模式的吉祥物用）。
    pub idle_since: Option<Instant>,
    /// 上一次的健康狀態，用來判斷「剛從吃緊恢復」。
    pub was_strained: bool,
    /// 上一次取樣時算出來的氣氛。畫面每一幀只讀它，不重算。
    current_mood: crate::ui::visual::mascot::Mood,
    /// CPU 頁的每核顯示方式：`false` 是逐條列，`true` 是熱度圖。
    ///
    /// 兩種都留著是因為它們回答不同的問題：逐條列給得出每顆核心的頻率，
    /// 熱度圖給得出「忙的是哪幾顆」的空間分佈。核心一多，逐條列就放不下，
    /// 那時自動改用熱度圖；`v` 可以隨時手動切換。
    pub cpu_heatmap: bool,
    /// 品牌識別（從 config 算一次）。
    pub brand: crate::ui::visual::logo::Brand,
    /// 上一幀有沒有真的把吉祥物畫出來。
    ///
    /// 大部分頁面沒有裝飾槽，小終端機也放不下 —— 那些情況下動畫時鐘
    /// 完全沒有理由跑。不看這個旗標的話，`decorations != off` 就足以
    /// 讓它每秒醒來三次、重畫整個畫面，而畫面上根本沒有東西在動。
    mascot_drawn: std::cell::Cell<bool>,
    /// 上一幀常駐浮水印畫在哪裡。
    ///
    /// 位置是每幀依畫面內容找出來的，但**找得到舊位置就沿用**：
    /// 換頁時剪影留在原地，那一區的格子完全不用重畫（量過：兩頁都畫得下
    /// 而位置相同時，換頁的邊際成本是 0 格；位置變動則要 540 格）。
    /// 視覺上也不會看到牠在頁與頁之間瞬移。
    mascot_spot: std::cell::Cell<Option<ratatui::layout::Rect>>,
    /// 說明頁捲到第幾行，以及最多能捲到哪裡（由畫的人回報）。
    ///
    /// 說明頁原本不能捲，內容超出視窗就**直接截斷而且沒有任何提示** ——
    /// 於是排在最後面的鍵（例如彩蛋的 `g`）在任何尺寸下都看不到，
    /// 小終端上連「q 離開」都被切掉。
    pub help_scroll: u16,
    help_max_scroll: std::cell::Cell<u16>,
    /// 目前這一幀，被選到的那份清單最多能捲到哪裡。
    ///
    /// 只有畫的人知道 —— 「還有幾列沒顯示」要等版面算完才成立。
    /// 沒有這個上界的話，在清單底部按 ↓ 會讓 `scroll` 一直累加而畫面
    /// 完全不動：使用者按了十下什麼都沒發生，然後要按十下 ↑ 才回得來。
    /// （Explain 視窗上一輪就是為了同一件事加了 `explain_max_scroll`。）
    scroll_bound: std::cell::Cell<usize>,
    /// 彩蛋遊戲。`None` 代表沒在玩 —— 那時它一次都不會讓主迴圈醒來。
    pub dino: Option<crate::ui::dino::Dino>,
    /// 這次執行的最佳分數。只放在記憶體裡。
    dino_best: u32,
    /// 排行榜放在哪（啟動時決定一次，只 stat 不建目錄）。
    pub scores: crate::scoreboard::Store,
    /// 死掉之後的狀態：榜、名字輸入框、結果訊息。重來或關掉就清掉。
    pub dino_over: Option<DinoOver>,
    /// 這一幀的裝飾層級與密度。由 `draw()` 依整個畫面的大小設定。
    decoration: std::cell::Cell<crate::ui::visual::Decoration>,
    density: std::cell::Cell<crate::ui::visual::Density>,

    /// 說明視窗最多能往下捲幾行。由上一幀繪製時算出來寫回來的 ——
    /// 真正的行數要等文字依視窗寬度折過行才知道，那是 UI 才有的資訊。
    pub explain_max_scroll: std::cell::Cell<u16>,
    pub status: Option<(String, Instant)>,
    /// 啟動時的提醒，留得比一般狀態訊息久（例如跑的是被系統版遮住的私人版）。
    pub advisory: Option<(String, Instant)>,
    pub quit: bool,
    pub started: Instant,
    /// 每一幀重建的可選區域。頁面在繪製時登記自己畫了什麼，
    /// 焦點才能直接框在畫面上的東西，而不是靠一份對不上的名詞清單。
    pub regions: crate::ui::focus::FocusRegistry,
    tx: Sender<WorkerMsg>,
    rx: Receiver<WorkerMsg>,
}

impl App {
    pub fn new(config: Config, depth: ColorDepth) -> Self {
        let interval = config.interval_duration();
        let theme = Theme::new(&config.theme, depth);
        let (tx, rx) = mpsc::channel();
        let view = View::parse(&config.default_page).unwrap_or(View::Overview);
        let brand =
            crate::ui::visual::logo::Brand::new(&config.branding.name, &config.branding.logo);
        // decorations = "off" 的人不想要任何裝飾，啟動畫面也是裝飾。
        let splash = if config.branding.splash && config.visual.decorations != "off" {
            Modal::Splash
        } else {
            Modal::None
        };
        let sort = match config.process_sort.as_str() {
            "rss" | "memory" => SortKey::Memory,
            "pid" => SortKey::Pid,
            "thr" | "threads" => SortKey::Threads,
            "user" => SortKey::User,
            _ => SortKey::Cpu,
        };
        Self {
            state: SystemState::with_options(
                config.history,
                Intervals::from_base(interval),
                config.gpu,
            ),
            config,
            theme,
            view,
            privilege: PrivilegeClient::new(),
            admin: AdminState::default(),
            paused: false,
            interval,
            sort,
            filter: String::new(),
            filter_editing: false,
            scroll: 0,
            focus: None,
            modal: splash,
            anim_frame: 0,
            last_anim: Instant::now(),
            idle_since: None,
            was_strained: false,
            current_mood: crate::ui::visual::mascot::Mood::Calm,
            cpu_heatmap: false,
            brand,
            mascot_drawn: std::cell::Cell::new(true),
            mascot_spot: std::cell::Cell::new(None),
            help_scroll: 0,
            help_max_scroll: std::cell::Cell::new(0),
            scroll_bound: std::cell::Cell::new(0),
            dino: None,
            dino_best: 0,
            scores: crate::scoreboard::Store::locate(),
            dino_over: None,
            decoration: std::cell::Cell::new(crate::ui::visual::Decoration::None),
            density: std::cell::Cell::new(crate::ui::visual::Density::Compact),
            explain_scroll: 0,
            explain_max_scroll: std::cell::Cell::new(0),
            status: None,
            advisory: None,
            quit: false,
            started: Instant::now(),
            regions: Default::default(),
            tx,
            rx,
        }
    }

    pub fn note(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// 狀態列訊息還在有效期內嗎。
    pub fn status_text(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, t)| t.elapsed() < Duration::from_secs(4))
            .map(|(s, _)| s.as_str())
            .or_else(|| {
                self.advisory
                    .as_ref()
                    .filter(|(_, t)| t.elapsed() < Duration::from_secs(20))
                    .map(|(s, _)| s.as_str())
            })
    }

    /// 啟動時的提醒：顯示 20 秒，而且不會被一般的狀態訊息蓋掉太久。
    pub fn advise(&mut self, msg: impl Into<String>) {
        self.advisory = Some((msg.into(), Instant::now()));
    }

    /// 取樣一輪。回傳 `true` 代表這次真的取到新資料（畫面需要重繪）。
    pub fn tick(&mut self, now: Instant) -> bool {
        let sampled = if self.paused {
            false
        } else {
            self.state.sample(now)
        };
        let had_worker = self.drain_worker();
        // 取樣完才重算氣氛：吉祥物的姿態跟著**資料**走，不跟著畫面更新走
        if sampled {
            self.refresh_mood();
        }
        sampled || had_worker
    }

    /// 距離下一次該取樣還有多久。主迴圈靠這個決定要睡多久，
    /// 而不是固定以某個 fps 空轉。
    pub fn next_sample_in(&self, now: Instant) -> Duration {
        if self.paused {
            return Duration::from_millis(250);
        }
        self.state.next_due(now)
    }

    fn drain_worker(&mut self) -> bool {
        let mut any = false;
        while let Ok(msg) = self.rx.try_recv() {
            any = true;
            match msg {
                WorkerMsg::Admin { tab, result } => {
                    *self.admin.slot(tab) = match result {
                        Ok(v) => Async::Ready(v),
                        Err(e) => Async::Failed(e),
                    };
                }
                WorkerMsg::Detail { user, result } => {
                    self.admin.detail_user = Some(user);
                    self.admin.detail = match result {
                        Ok(v) => Async::Ready(v),
                        Err(e) => Async::Failed(e),
                    };
                }
                WorkerMsg::Mutation {
                    description,
                    result,
                } => match result {
                    Ok(_) => self.note(format!("完成：{description}")),
                    Err(e) => {
                        self.modal = Modal::Message {
                            title: "操作失敗".into(),
                            body: format!("{description}\n\n{e}"),
                        }
                    }
                },
            }
        }
        any
    }

    /// 目前框住的那一塊。分頁層時是 `None`。
    pub fn focused_region(&self) -> Option<crate::ui::focus::FocusRegion> {
        self.regions.get(self.focus?)
    }

    /// 目前在同一層兄弟裡排第幾 / 共幾個。
    pub fn focus_position(&self) -> (usize, usize) {
        let Some(f) = self.focus else { return (0, 0) };
        let sib = self.regions.siblings_of(f);
        let pos = sib.iter().position(|&i| i == f).map_or(0, |p| p + 1);
        (pos, sib.len())
    }

    /// 目前的深度（分頁層是 0，第一層面板是 1，以此類推）。
    pub fn focus_depth(&self) -> usize {
        match self.focus {
            None => 0,
            Some(f) => self.regions.depth_of(f) + 1,
        }
    }

    /// 麵包屑：從最上層到目前這一格的標題。
    pub fn focus_path(&self) -> Vec<String> {
        let Some(f) = self.focus else {
            return Vec::new();
        };
        self.regions
            .path_to(f)
            .into_iter()
            .filter_map(|i| self.regions.get(i).map(|r| r.label))
            .collect()
    }

    /// 目前這一格還能不能再往下鑽。
    pub fn can_descend(&self) -> bool {
        match self.focus {
            None => !self.regions.is_empty(),
            Some(f) => !self.regions.children_of(f).is_empty(),
        }
    }

    /// 目前框住的東西要怎麼解釋。
    pub fn describe_target(&self) -> Option<crate::metrics::describe::DescribeTarget> {
        self.focused_region().map(|r| r.target)
    }

    /// 目前選到的 metric id（只有在框住的是固定 metric 時才有值）。
    pub fn focused_metric(&self) -> Option<&'static str> {
        match self.focused_region()?.target {
            crate::metrics::describe::DescribeTarget::Metric(id) => Some(id),
            _ => None,
        }
    }

    // ── 管理員操作 ──────────────────────────────────────────────────────
    /// 觸發目前分頁的查詢。已經在跑就不重複觸發。
    pub fn request_admin(&mut self, force: bool) {
        // 不用快取狀態擋 —— 憑證可能剛好過期或剛好被授予，
        // 只有實際打過去才知道。唯一的例外是 helper 根本不存在。
        if matches!(self.privilege.state(), PrivilegeState::HelperMissing(_)) {
            return;
        }
        let tab = self.admin.tab();
        let slot = self.admin.slot(tab);
        if slot.is_running() || (!force && slot.ready().is_some()) {
            return;
        }
        *slot = Async::Running {
            since: Instant::now(),
        };
        self.spawn_admin(tab);
    }

    fn spawn_admin(&self, tab: AdminTab) {
        let tx = self.tx.clone();
        let op = tab.operation();
        // clone 出來的 client 共用同一份授權狀態
        let client = self.privilege.clone();
        // 每個查詢都在自己的執行緒跑，不會卡住 UI；執行緒數有上限
        // （最多四個分頁各一個），不會無限增長。
        std::thread::spawn(move || {
            let result = client.run(&op).map_err(|e| e.to_string());
            let _ = tx.send(WorkerMsg::Admin { tab, result });
        });
    }

    /// 展開某個使用者的儲存明細。
    pub fn request_storage_detail(&mut self, user: String) {
        if !self.privilege.state().is_available() || self.admin.detail.is_running() {
            return;
        }
        self.admin.detail = Async::Running {
            since: Instant::now(),
        };
        self.admin.detail_user = Some(user.clone());
        let tx = self.tx.clone();
        let client = self.privilege.clone();
        std::thread::spawn(move || {
            let result = client
                .run(&Operation::StorageUserDetail { user: user.clone() })
                .map_err(|e| e.to_string());
            let _ = tx.send(WorkerMsg::Detail { user, result });
        });
    }

    /// 真正執行一個已經過二次確認的變更操作。
    pub fn execute_confirmed(&mut self, c: Confirmation) {
        let tx = self.tx.clone();
        let op = c.operation.clone();
        let client = self.privilege.clone();
        let description = format!("{} (PID {})", c.action, c.target_pid);
        std::thread::spawn(move || {
            let result = client.run(&op).map_err(|e| e.to_string());
            let _ = tx.send(WorkerMsg::Mutation {
                description,
                result,
            });
        });
    }

    pub fn privilege_label(&self) -> String {
        self.privilege.state().label()
    }
}

/// 恐龍死掉之後的畫面狀態。
///
/// `typed` 是 `Some` 就代表名字輸入框開著，所有按鍵都是在填名字；
/// 送出或 Esc 之後變回 `None`，`note` 說明結果。
#[derive(Debug, Clone, Default)]
pub struct DinoOver {
    pub board: crate::scoreboard::Board,
    pub typed: Option<String>,
    pub note: Option<String>,
}

/// 從按鍵到狀態轉移。抽成獨立函式是為了能在沒有終端機的情況下測試。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Tab,
    BackTab,
    Enter,
    Esc,
    Backspace,
}

impl App {
    /// 目前開著的視窗要不要吃掉這個鍵。
    ///
    /// `None` 只在**完全沒有視窗**時回傳 —— 只要有視窗開著，這個鍵就
    /// 到此為止，不會傳到底下的頁面。這是「modal owns the input」：
    /// 使用者按下的第一個鍵是在對視窗說話，不是在對 dashboard 說話。
    fn on_key_modal(&mut self, key: Key) -> Option<SideEffect> {
        match &self.modal {
            Modal::None => None,
            Modal::Confirm(c) => Some(self.on_key_confirm(key, c.clone())),
            Modal::Dino => {
                // 名字輸入框開著時，所有鍵都是在填名字：r 是字母、Enter 送出、
                // Esc 不記錄（不是關閉）。不填就不記 —— 空的 Enter 當沒按。
                if self.dino_over.as_ref().is_some_and(|o| o.typed.is_some()) {
                    match key {
                        Key::Enter => self.submit_dino_name(),
                        Key::Esc => {
                            if let Some(o) = self.dino_over.as_mut() {
                                o.typed = None;
                                o.note = Some("沒有記錄".into());
                            }
                        }
                        Key::Backspace => {
                            if let Some(t) = self.dino_over.as_mut().and_then(|o| o.typed.as_mut())
                            {
                                t.pop();
                            }
                        }
                        Key::Char(c) if !c.is_control() => {
                            if let Some(t) = self.dino_over.as_mut().and_then(|o| o.typed.as_mut())
                            {
                                let mut next = t.clone();
                                next.push(c);
                                if crate::ui::format::width(&next) <= crate::scoreboard::NAME_WIDTH
                                {
                                    *t = next;
                                }
                            }
                        }
                        _ => {}
                    }
                    return Some(SideEffect::None);
                }
                // 遊戲擁有全部按鍵：跳、蹲、重來都不能同時去暫停取樣或換頁
                match key {
                    Key::Esc | Key::Char('q') => {
                        self.modal = Modal::None;
                        self.dino = None;
                        self.dino_over = None;
                    }
                    _ => {
                        if let Some(g) = self.dino.as_mut() {
                            g.on_key(key, Instant::now());
                            if !g.is_over() {
                                // 重來了：榜與訊息屬於上一局
                                self.dino_over = None;
                            }
                        }
                    }
                }
                Some(SideEffect::None)
            }
            Modal::Explain { .. } => {
                // Explain 的內容常常比畫面長，↑↓ 用來捲動而不是關閉
                match key {
                    // 捲到底就停住。沒有上界的話往下按會捲進一片空白，
                    // 而且要按同樣多次 ↑ 才回得來。
                    Key::Up => self.explain_scroll = self.explain_scroll.saturating_sub(1),
                    Key::Down => self.explain_scroll = self.scroll_explain(1),
                    Key::PageUp => self.explain_scroll = self.explain_scroll.saturating_sub(10),
                    Key::PageDown => self.explain_scroll = self.scroll_explain(10),
                    _ => {
                        self.modal = Modal::None;
                        self.explain_scroll = 0;
                    }
                }
                Some(SideEffect::None)
            }
            // 說明頁跟 Explain 一樣可以捲：內容比視窗長是常態，
            // 截斷而不說是最糟的做法（彩蛋的 `g` 就是這樣消失的）。
            Modal::Help => {
                let max = self.help_max_scroll.get();
                match key {
                    Key::Up => self.help_scroll = self.help_scroll.saturating_sub(1),
                    Key::Down => self.help_scroll = (self.help_scroll + 1).min(max),
                    Key::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                    Key::PageDown => self.help_scroll = (self.help_scroll + 10).min(max),
                    _ => {
                        self.modal = Modal::None;
                        self.help_scroll = 0;
                    }
                }
                Some(SideEffect::None)
            }
            // 啟動畫面 / 訊息：任何鍵都只做一件事 —— 關掉它。
            //
            // 「順便讓這個鍵生效」聽起來體貼，實際上是穿透：
            // 空白會暫停取樣、方向鍵會移動焦點、1 會換頁。
            // 使用者看到的是一張蓋住畫面的東西，他按鍵是想把它關掉。
            Modal::Splash | Modal::Message { .. } => {
                self.modal = Modal::None;
                Some(SideEffect::None)
            }
        }
    }

    /// 處理一個按鍵。回傳主迴圈需要代為執行的副作用。
    pub fn on_key(&mut self, key: Key) -> SideEffect {
        // 篩選輸入模式吃掉大部分按鍵
        if self.filter_editing {
            match key {
                Key::Esc => {
                    self.filter.clear();
                    self.filter_editing = false;
                }
                Key::Enter => self.filter_editing = false,
                Key::Backspace => {
                    self.filter.pop();
                }
                Key::Char(c) if !c.is_control() => self.filter.push(c),
                _ => {}
            }
            self.scroll = 0;
            return SideEffect::None;
        }

        // 視窗優先處理：**視窗擁有輸入**。
        //
        // 這裡回傳 `Some` 就代表這個鍵屬於視窗，底下的頁面完全收不到。
        // 寫成回傳 Option 而不是在 match 裡各自 return，是因為原本就是
        // 那樣寫的，然後 `Modal::Splash` 那一支忘了 return —— 於是開機
        // 畫面按空白會「關掉啟動畫面**並且**暫停取樣」，按 1 會直接跳到
        // CPU 頁。少一個 return 就漏一個穿透，而且看不出來。
        if let Some(e) = self.on_key_modal(key) {
            return e;
        }

        match key {
            // Esc 是統一的「回到上一層」，退到底就是回總覽。
            //
            // 到了總覽的分頁層就停住，**不會**離開程式：說明上寫的是
            // 「Esc 回到上一層、q 離開」，連按 Esc 退層時不該把工具關掉。
            Key::Esc | Key::Backspace => {
                if !self.ascend() {
                    if self.view != View::Overview {
                        self.view = View::Overview;
                        self.reset_page_state();
                    } else {
                        self.note("已經在最上層了。按 q 離開。".to_owned());
                    }
                }
            }
            Key::Char('q') => {
                self.quit = true;
                return SideEffect::Quit;
            }
            Key::Char('?') | Key::Char('h') => self.modal = Modal::Help,
            // 彩蛋。`g` = game，目前沒被用掉，而且不是破壞性操作 ——
            // 按錯了按 Esc 就回來，資料一格都沒動。
            Key::Char('g') => {
                self.dino = Some(crate::ui::dino::Dino::new(Instant::now(), self.dino_best));
                self.modal = Modal::Dino;
            }
            // e 解釋「目前框起來的那一塊」。看到什麼就解釋什麼。
            Key::Char('e') => {
                if let Some(target) = self.describe_target() {
                    self.explain_scroll = 0;
                    self.modal = Modal::Explain { target };
                }
            }
            Key::Char('`') | Key::Char('~') | Key::Char('0') => {
                self.view = View::Overview;
                self.reset_page_state();
            }
            Key::Char(c @ '1'..='6') => {
                self.view = VIEWS[(c as u8 - b'0') as usize];
                self.reset_page_state();
            }
            Key::Char('A') | Key::Char('a') => {
                self.view = View::Admin;
                self.reset_page_state();
                if self.privilege.state().is_available() {
                    self.request_admin(false);
                }
            }
            // Tab 換頁（在任何一層都有效）；←→ 只在分頁層才是換頁
            Key::Tab => {
                let i = VIEWS.iter().position(|v| *v == self.view).unwrap_or(0);
                self.view = VIEWS[(i + 1) % VIEWS.len()];
                self.reset_page_state();
            }
            Key::BackTab => {
                let i = VIEWS.iter().position(|v| *v == self.view).unwrap_or(0);
                self.view = VIEWS[(i + VIEWS.len() - 1) % VIEWS.len()];
                self.reset_page_state();
            }
            Key::Char(' ') => {
                self.paused = !self.paused;
                let s = if self.paused {
                    "已暫停取樣"
                } else {
                    "已繼續取樣"
                };
                self.note(s);
            }
            Key::Char('+') | Key::Char('=') => self.change_interval(-0.2),
            Key::Char('-') | Key::Char('_') => self.change_interval(0.2),
            Key::Char('r') => {
                if self.view == View::Admin {
                    self.request_admin(true);
                    self.note("重新查詢中…");
                }
            }
            Key::Char('s') if self.view == View::Process => {
                self.sort = self.sort.next();
                self.note(format!("排序：{}", self.sort.label()));
            }
            Key::Char('/') if self.view == View::Process => {
                self.filter_editing = true;
                self.filter.clear();
            }
            Key::Char('u') => {
                // 先立刻重新確認一次（sudo -n，幾毫秒）。
                // 直接信任快取的話會發生「說已解鎖 → 查詢卻失敗 → 再按一次
                // 才跳密碼」，那正是使用者回報的問題。
                return match self.privilege.recheck() {
                    PrivilegeState::Locked => SideEffect::AuthenticateSudo,
                    PrivilegeState::Available => {
                        // 已經有授權了就直接帶去管理員頁並開始查詢 ——
                        // 只印一行「已解鎖」但畫面什麼都沒變，等於沒反應。
                        self.view = View::Admin;
                        self.reset_page_state();
                        self.request_admin(false);
                        self.note("管理員功能已解鎖，查詢中…");
                        SideEffect::None
                    }
                    s => {
                        self.modal = Modal::Message {
                            title: "無法使用管理員功能".into(),
                            body: s
                                .detail()
                                .unwrap_or("你沒有執行這個管理員功能的權限。")
                                .to_owned(),
                        };
                        SideEffect::None
                    }
                };
            }
            Key::Char('m') => {
                // 在介面裡換吉祥物，不必改設定檔再重開。
                // 這只是畫面上的東西，不寫回設定 —— 想固定下來就寫 config。
                const CYCLE: [&str; 3] = ["fox", "deer", "none"];
                let i = CYCLE
                    .iter()
                    .position(|s| *s == self.config.branding.mascot)
                    // auto 目前就是狐狸，所以從狐狸的下一個開始
                    .unwrap_or(0);
                let next = CYCLE[(i + 1) % CYCLE.len()];
                self.config.branding.mascot = next.to_owned();
                self.note(match next {
                    "none" => "吉祥物：關閉".to_owned(),
                    other => format!("吉祥物：{other}（想固定就寫進 config）"),
                });
            }
            Key::Char('v') if self.view == View::Cpu => {
                self.cpu_heatmap = !self.cpu_heatmap;
                self.note(if self.cpu_heatmap {
                    "每核顯示：熱度圖".to_owned()
                } else {
                    "每核顯示：逐條列".to_owned()
                });
            }
            Key::Char('t') => {
                // 循環主題
                let i = crate::theme::THEMES
                    .iter()
                    .position(|t| *t == self.theme.name)
                    .unwrap_or(0);
                let next = crate::theme::THEMES[(i + 1) % crate::theme::THEMES.len()];
                self.theme = Theme::new(next, self.theme.depth);
                self.note(format!("主題：{next}"));
            }
            Key::Up => self.navigate(crate::ui::focus::Dir::Up),
            Key::Down => self.navigate(crate::ui::focus::Dir::Down),
            Key::Left => self.navigate(crate::ui::focus::Dir::Left),
            Key::Right => self.navigate(crate::ui::focus::Dir::Right),
            Key::PageUp => self.page_scroll(-10),
            Key::PageDown => self.page_scroll(10),
            Key::Home => {
                self.scroll = 0;
                self.focus = None;
            }
            Key::End => self.scroll = usize::MAX / 2,
            // Enter：往下鑽一層。層數不設限，頁面登記多深就能鑽多深。
            Key::Enter => {
                // 焦點已經停在使用者那一列時，Enter 就是「展開這個人的明細」
                // `list_index` 只有清單裡的列才有，分頁本身沒有 ——
                // 用它判斷才不會在分頁上按 Enter 就跳去展開明細。
                let on_user_row = self.view == View::Admin
                    && self.admin.tab() == AdminTab::Storage
                    && self
                        .focus
                        .and_then(|f| self.regions.get(f))
                        .is_some_and(|r| r.list_index.is_some());
                if on_user_row {
                    if let Some(user) = self.selected_storage_user() {
                        self.request_storage_detail(user);
                    }
                } else {
                    self.descend();
                }
            }
            Key::Char('[') if self.view == View::Admin => {
                self.admin.tab_index =
                    (self.admin.tab_index + ADMIN_TABS.len() - 1) % ADMIN_TABS.len();
                self.admin.selected = 0;
                self.request_admin(false);
            }
            Key::Char(']') if self.view == View::Admin => {
                self.admin.tab_index = (self.admin.tab_index + 1) % ADMIN_TABS.len();
                self.admin.selected = 0;
                self.request_admin(false);
            }
            _ => {}
        }
        SideEffect::None
    }

    fn on_key_confirm(&mut self, key: Key, mut c: Confirmation) -> SideEffect {
        match key {
            Key::Esc | Key::Char('n') | Key::Char('q') => self.modal = Modal::None,
            Key::Left | Key::Right | Key::Tab => {
                c.confirmed_selected = !c.confirmed_selected;
                self.modal = Modal::Confirm(c);
            }
            Key::Char('y') => {
                // 就算按 y，也要先把選項移到「確認」再按 Enter —— 避免手滑
                c.confirmed_selected = true;
                self.modal = Modal::Confirm(c);
            }
            Key::Enter => {
                if c.confirmed_selected {
                    self.modal = Modal::None;
                    self.execute_confirmed(c);
                } else {
                    self.modal = Modal::None;
                    self.note("已取消");
                }
            }
            _ => {}
        }
        SideEffect::None
    }

    fn reset_page_state(&mut self) {
        self.scroll = 0;
        // 換頁時退回分頁層：新的一頁還沒畫，舊的索引沒有意義
        self.focus = None;
    }

    /// 方向鍵。
    ///
    /// 分頁層：←→ 換頁、↓ 進入頁面。
    /// 有焦點時：在**同一層的兄弟之間**移動，層數不設限。
    /// 焦點所在的那一層若屬於可捲動的清單，撞到頭尾時改成捲動。
    fn navigate(&mut self, dir: crate::ui::focus::Dir) {
        use crate::ui::focus::Dir;

        // 分頁層
        let Some(cur) = self.focus else {
            match dir {
                Dir::Left => self.switch_page(-1),
                Dir::Right => self.switch_page(1),
                Dir::Down => self.descend(),
                Dir::Up => {}
            }
            return;
        };

        let regions = self.regions.all();
        let siblings = self.regions.siblings_of(cur);
        if siblings.is_empty() {
            return;
        }
        if let Some(next) = crate::ui::focus::nearest(&regions, &siblings, cur, dir) {
            self.focus = Some(next);
            self.sync_admin_focus();
            return;
        }

        // 同方向沒有兄弟了。若這一層是清單，就捲動；否則繞回去。
        let in_list = self
            .regions
            .parent_of(cur)
            .and_then(|p| self.regions.get(p))
            .is_some_and(|p| p.scrollable);
        if in_list && matches!(dir, Dir::Up | Dir::Down) {
            let delta: i64 = if dir == Dir::Down { 1 } else { -1 };
            // 捲得動就捲、到頭尾就繞到另一端 —— 每一次按鍵都要在畫面上
            // 看得到結果，不能有「按了但什麼都沒變」的情況。
            match self.scroll_focused(delta) {
                Some(true) => {
                    self.focus_list_end(dir == Dir::Down);
                    return;
                }
                Some(false) => return,
                // 只有一列（或不是清單）：往下走兄弟之間繞回去的規則
                None => {}
            }
        }
        if matches!(dir, Dir::Up | Dir::Down) {
            // 只在兄弟們真的上下排列時才繞回去。
            //
            // 排成一排的東西（Admin 的分頁列）沒有「上一個」「下一個」，
            // 繞了只會在分頁之間打轉出不去 —— 那正是「上下鍵在三四循環卡死」。
            let stacked = self
                .regions
                .get(cur)
                .zip(siblings.iter().filter_map(|&i| self.regions.get(i)).next())
                .is_some()
                && siblings
                    .iter()
                    .filter_map(|&i| self.regions.get(i))
                    .any(|r| Some(r.rect.y) != self.regions.get(cur).map(|c| c.rect.y));
            if stacked {
                if let Some(w) = crate::ui::focus::wrap_within(&siblings, cur, dir == Dir::Down) {
                    self.focus = Some(w);
                }
                return;
            }
            // 這一層是橫排的，往下就是往裡走一層（分頁列 → 那個分頁的清單）
            if dir == Dir::Down {
                self.descend();
            }
        }
        // 左右到底就停著，不繞 —— 繞了反而讓人以為跳到別的地方
    }

    /// 這一幀的裝飾層級。
    ///
    /// 由**整個畫面**的大小決定，在 `draw()` 開頭算一次存起來。
    /// 不能各自拿手邊的子區域去算 —— 頁首只有兩列高，那樣算出來永遠是
    /// 「太小，不畫裝飾」，chip 就永遠不會出現（真的踩過）。
    pub fn deco(&self) -> crate::ui::visual::Decoration {
        self.decoration.get()
    }

    /// 常駐浮水印用的裝飾等級。
    ///
    /// 跟 [`App::deco`] 不一樣：那個等級是「這台終端機還剩多少餘裕給裝飾」，
    /// 80 欄以下會變成 `None`，因為 logo、chip 這些東西**會佔掉資料的空間**。
    /// 浮水印不會 —— 它只填本來就空白的格子，露多少由乾淨區域決定。
    /// 所以這裡只問一件事：使用者有沒有把裝飾關掉。
    /// 拿尺寸等級去擋它的結果是：80 欄的終端機明明有一塊 36×7 的空白，
    /// 連一個頭都不給露。
    pub fn watermark_deco(&self) -> crate::ui::visual::Decoration {
        use crate::ui::visual::Decoration;
        if self.config.visual.decorations == "off" {
            Decoration::None
        } else {
            match self.deco() {
                Decoration::None => Decoration::Minimal,
                d => d,
            }
        }
    }

    /// 這一幀的版面密度。同樣由整個畫面決定。
    pub fn dens(&self) -> crate::ui::visual::Density {
        self.density.get()
    }

    /// 在 `draw()` 開頭呼叫一次，把這一幀的視覺層級定下來。
    pub fn set_frame_metrics(&self, area: ratatui::layout::Rect) {
        self.decoration.set(crate::ui::visual::Decoration::resolve(
            &self.config.visual.decorations,
            area,
        ));
        self.density.set(crate::ui::visual::Density::resolve(
            &self.config.visual.density,
            area,
        ));
    }

    /// 組成類長條要用的 pattern。
    pub fn pattern(&self) -> crate::ui::visual::pattern::Pattern {
        crate::ui::visual::pattern::Pattern::parse(&self.config.visual.pattern).unwrap_or_default()
    }

    /// 一頁在頁首上要顯示的名字。
    ///
    /// 有自訂別名時是 `BRAIN · CPU`；畫面窄的時候別名讓位給原名 ——
    /// 別名是給熟悉這台機器的人看的，原名是給所有人看的。
    pub fn page_title(&self, view: View, width: u16) -> String {
        let canonical = view.title();
        if width < 150 {
            return canonical.to_owned();
        }
        self.config.labels.decorate(canonical)
    }

    /// 面板標題上的別名版本（給頁面內部的面板用）。
    pub fn label(&self, canonical: &str, width: u16) -> String {
        if width < 100 {
            canonical.to_owned()
        } else {
            self.config.labels.decorate(canonical)
        }
    }

    /// 推進吉祥物的動畫格。回傳 `true` 代表畫面需要重畫。
    ///
    /// 這是**唯一**會讓動畫影響主迴圈的地方，而它只碰 `anim_frame`。
    /// 不論動畫跑多快，collector 的排程完全不受影響 ——
    /// 裝飾動起來不該讓機器多讀一次 `/proc`。
    pub fn tick_animation(&mut self, now: Instant) -> bool {
        if !self.decorations_animate() {
            return false;
        }
        let fps = self.config.branding.mascot_fps.clamp(1, 8) as u32;
        let period = Duration::from_micros(1_000_000 / fps as u64);
        if now.duration_since(self.last_anim) < period {
            return false;
        }
        self.last_anim = now;
        self.anim_frame = self.anim_frame.wrapping_add(1);
        true
    }

    /// 這一輪需不需要跑動畫。
    ///
    /// 三個條件都要成立：沒關掉裝飾、有選吉祥物、而且**上一幀真的畫出來了**。
    /// 最後那個是關鍵 —— 八個頁面裡只有總覽有裝飾槽，小終端機更是完全放不下。
    /// 少了它，動畫會在畫面上什麼都沒動的情況下每秒重畫三次。
    pub fn decorations_animate(&self) -> bool {
        self.config.visual.decorations != "off"
            && self.config.branding.mascot != "none"
            && self.mascot_drawn.get()
    }

    /// 每一幀開始時重設。回報的責任在 `slot::mascot` 那邊，
    /// 因為只有它知道「這塊空間畫不畫得下」。
    pub fn begin_frame(&self) {
        self.mascot_drawn.set(false);
        // 預設 0：沒有人回報就代表「這份清單根本不會捲」。
        // 這個預設很重要 —— 有幾個面板登記成清單卻從來沒實作捲動，
        // 少了它就會變成看不見的狀態黑洞。
        self.scroll_bound.set(0);
    }

    /// 說明頁最多能捲到哪裡。畫的人回報。
    pub fn help_max_scroll(&self) -> &std::cell::Cell<u16> {
        &self.help_max_scroll
    }

    /// 畫清單的人回報「最多捲到哪裡」。
    pub fn set_scroll_bound(&self, max: usize) {
        self.scroll_bound.set(max);
    }

    /// 給 `slot::mascot` 與 `watermark::render` 回報用。
    pub fn mascot_flag(&self) -> &std::cell::Cell<bool> {
        &self.mascot_drawn
    }

    /// 常駐浮水印上一次畫在哪裡，讓這一幀優先沿用同一個位置。
    pub fn mascot_spot(&self) -> &std::cell::Cell<Option<ratatui::layout::Rect>> {
        &self.mascot_spot
    }

    /// 現在該畫哪一隻、哪個姿態。
    ///
    /// 集中在這裡而不是散在各個畫面：姿態是由**系統狀態**決定的，
    /// 不該讓每個呼叫端各自拼一次條件。
    pub fn mascot_now(
        &self,
    ) -> Option<(
        crate::ui::visual::mascot::Species,
        crate::ui::visual::mascot::State,
    )> {
        use crate::ui::visual::mascot::{self, Mood};
        // 暫停取樣時一律休息。畫面已經凍結，動作卻還在跑會讓人
        // 以為資料仍在更新；而且暫停時 refresh_mood 根本不會被呼叫，
        // 沿用舊的氣氛只會停在一個過期的姿態上。
        let mood = if self.paused {
            Mood::Idle
        } else {
            self.current_mood
        };
        mascot::choose(
            &self.config.branding.mascot,
            &self.config.branding.mascot_mode,
            &self.config.branding.mascot_state,
            mood,
            self.is_idle() || self.paused,
        )
    }

    /// 推進彩蛋遊戲。回傳 true 代表畫面要重畫。
    ///
    /// 跟動畫時鐘不同，遊戲**不會**在使用者操作時讓位 —— 遊戲本身就是
    /// 操作。也跟 collector 完全無關：這個函式碰不到任何取樣狀態。
    pub fn tick_game(&mut self, now: Instant) -> bool {
        match self.dino.as_mut() {
            Some(g) => {
                let changed = g.tick(now);
                self.dino_best = self.dino_best.max(g.best());
                if g.is_over() && self.dino_over.is_none() {
                    // 死掉的那一格：讀榜（幾個小檔），嚴格高於第五名才開輸入框
                    let board = self.scores.load();
                    let typed = board.qualifies(g.score()).then(String::new);
                    self.dino_over = Some(DinoOver {
                        board,
                        typed,
                        note: None,
                    });
                }
                changed
            }
            None => false,
        }
    }

    /// 名字輸入框按下 Enter。
    fn submit_dino_name(&mut self) {
        use crate::scoreboard::{sanitize_name, Verdict};
        let Some(score) = self.dino.as_ref().map(|g| g.score()) else {
            return;
        };
        let Some(over) = self.dino_over.as_mut() else {
            return;
        };
        let name = sanitize_name(over.typed.as_deref().unwrap_or(""));
        if name.is_empty() {
            // 一定要填才記：空的 Enter 當沒按，輸入框留著
            return;
        }
        let note = match self.scores.record(&name, score) {
            Ok(Verdict::Recorded { rank }) => {
                format!("已記錄：{name} {score:05}，目前第 {rank} 名")
            }
            Ok(Verdict::NotHigher { existing }) => {
                format!("榜上的 {name} 已有 {existing:05}，這次 {score:05} 沒有更高，不記錄")
            }
            Err(e) => format!("寫入失敗，沒有記錄：{e}"),
        };
        over.typed = None;
        over.note = Some(note);
        over.board = self.scores.load();
    }

    /// 下一格遊戲畫面還要多久。沒在玩就回傳「很久以後」——
    /// 彩蛋關著的時候不該讓主迴圈多醒一次。
    pub fn next_game_tick_in(&self, now: Instant) -> Duration {
        match &self.dino {
            Some(g) if !g.is_over() => {
                crate::ui::dino::FRAME.saturating_sub(now.saturating_duration_since(g.last_tick()))
            }
            // 結束畫面是靜止的，不需要時鐘 —— 按鍵自己會觸發重畫
            _ => Duration::from_secs(3600),
        }
    }

    /// 下一次推進動畫還要多久。主迴圈用它決定睡多久。
    pub fn next_animation_in(&self, now: Instant) -> Duration {
        if !self.decorations_animate() {
            return Duration::from_secs(3600);
        }
        let fps = self.config.branding.mascot_fps.clamp(1, 8) as u64;
        Duration::from_micros(1_000_000 / fps).saturating_sub(now.duration_since(self.last_anim))
    }

    /// 重算系統現在的「氣氛」，給吉祥物挑姿態用。
    ///
    /// 在取樣之後算一次就好，畫面只讀 [`Self::current_mood`] ——
    /// 每一幀重算不但浪費，還會讓「剛恢復」這種狀態被畫面更新次數影響。
    /// 全部重用已經取樣好的狀態，**不會**為了裝飾多讀任何東西。
    pub fn refresh_mood(&mut self) -> crate::ui::visual::mascot::Mood {
        use crate::metrics::model::Severity;
        use crate::ui::visual::mascot::Mood;

        let (sev, _) = crate::ui::overall_health(self);
        let cpu = self.state.cpu.state().usage.get().unwrap_or(0.0);
        let strained = sev >= Severity::Warning;

        // 「剛恢復」是一個會消失的狀態：吃緊過、現在好了，就短暫顯示 return
        let mood = if strained {
            self.idle_since = None;
            Mood::Strained
        } else if self.was_strained {
            self.idle_since = None;
            Mood::Recovering
        } else if cpu < 5.0 {
            // 閒置要「持續」一段時間才算，不然 CPU 一有起伏姿態就在跳
            let since = *self.idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_secs(20) {
                Mood::Idle
            } else {
                Mood::Calm
            }
        } else {
            self.idle_since = None;
            if cpu >= 45.0 {
                Mood::Busy
            } else {
                Mood::Calm
            }
        };
        self.was_strained = strained;
        self.current_mood = mood;
        mood
    }

    /// 上一次取樣時算出來的氣氛。
    pub fn current_mood(&self) -> crate::ui::visual::mascot::Mood {
        self.current_mood
    }

    /// 系統是不是已經閒置夠久了。
    pub fn is_idle(&self) -> bool {
        self.idle_since
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(20))
    }

    /// 說明視窗往下捲，夾在上一幀算出來的上界。
    fn scroll_explain(&self, delta: u16) -> u16 {
        self.explain_scroll
            .saturating_add(delta)
            .min(self.explain_max_scroll.get())
    }

    /// Admin 頁的焦點與分頁 / 選取列同步。
    ///
    /// 分頁本身就是焦點樹的第一層，所以焦點移到哪個分頁，就切到那個分頁；
    /// 焦點移到清單的某一列，就選取那一列。使用者不必再記 `[` `]`。
    fn sync_admin_focus(&mut self) {
        if self.view != View::Admin {
            return;
        }
        let Some(f) = self.focus else { return };
        let Some(r) = self.regions.get(f) else { return };

        // 焦點在分頁上 → 切換分頁
        if r.parent.is_none() {
            if let Some(i) = ADMIN_TABS.iter().position(|t| t.title() == r.label) {
                if i != self.admin.tab_index {
                    self.admin.tab_index = i;
                    self.admin.selected = 0;
                    self.request_admin(false);
                }
            }
            return;
        }
        // 焦點在清單的某一列 → 選取那一列。
        // 用登記時記下的絕對索引，不能用「排第幾個兄弟」——
        // 只有看得到的列會被登記，清單捲動之後兩者會差一個視窗偏移量。
        if let Some(i) = r.list_index {
            self.admin.selected = i;
        }
    }

    /// 畫面重畫之後，把焦點對回目前選取的那一列。
    ///
    /// 每一幀都會重建 region 清單，清單捲動時同一個索引會落在不同的列上。
    /// 選取狀態（`admin.selected`）才是真相，焦點框要跟著它走。
    pub fn realign_focus(&mut self) {
        if self.view != View::Admin {
            return;
        }
        let Some(f) = self.focus else { return };
        let Some(parent) = self.regions.get(f).and_then(|r| r.parent) else {
            return;
        };
        if let Some(row) = self.regions.row_with_index(parent, self.admin.selected) {
            self.focus = Some(row);
        }
    }

    /// 往下鑽一層。
    fn descend(&mut self) {
        match self.focus {
            None => {
                self.focus = self.regions.roots().first().copied();
            }
            Some(f) => {
                if let Some(&first) = self.regions.children_of(f).first() {
                    self.focus = Some(first);
                }
            }
        }
        self.sync_admin_focus();
    }

    /// 往上退一層。回傳 `false` 代表已經在最上層。
    fn ascend(&mut self) -> bool {
        match self.focus {
            None => false,
            Some(f) => {
                self.focus = self.regions.parent_of(f);
                true
            }
        }
    }

    fn switch_page(&mut self, delta: i64) {
        let i = VIEWS.iter().position(|v| *v == self.view).unwrap_or(0) as i64;
        let n = VIEWS.len() as i64;
        self.view = VIEWS[((i + delta).rem_euclid(n)) as usize];
        self.reset_page_state();
    }

    /// 清單裡的上下鍵：捲得動就捲，捲到頭尾就**繞到另一端**。
    ///
    /// 回傳 `None` = 沒東西可捲（清單只有一列，或根本不是清單）；
    /// `Some(false)` = 往前走了；`Some(true)` = 繞到了另一端 —— 呼叫端要把
    /// 焦點框跟著搬到那一端的那一列。
    ///
    /// 只能有**一個**狀態在動。早期版本在底部按 ↓ 是看不見的狀態改變
    /// （`scroll` 一直累加）；後來改成捲不動就停、讓焦點框自己繞回第一列，
    /// 結果是箭頭跳到第一列、選取的反白還留在最後一列 —— 兩個狀態各走
    /// 各的，按住不放時焦點自己轉圈，放開才「真的」選到（使用者回報）。
    /// 現在繞的是選取（或捲動位置）本身，焦點框只是跟著它。
    fn scroll_focused(&mut self, delta: i64) -> Option<bool> {
        let (cur, max) = match self.view {
            View::Admin => (self.admin.selected, self.admin_list_len().saturating_sub(1)),
            _ => (self.scroll, self.scroll_bound.get()),
        };
        if max == 0 || delta == 0 {
            return None;
        }
        let cur = cur.min(max);
        let target = cur as i64 + delta;
        let (next, wrapped) = if target > max as i64 {
            if cur == max {
                (0, true)
            } else {
                (max, false)
            }
        } else if target < 0 {
            if cur == 0 {
                (max, true)
            } else {
                (0, false)
            }
        } else {
            (target as usize, false)
        };
        match self.view {
            View::Admin => self.admin.selected = next,
            _ => self.scroll = next,
        }
        Some(wrapped)
    }

    /// 繞到另一端之後把焦點框搬到那一端的那一列。
    ///
    /// 管理員頁畫完會再用 `realign_focus` 對回選取的那一列（可見的視窗
    /// 會重新以它為中心），這裡先搬一次是讓這一幀就看得到。
    fn focus_list_end(&mut self, start: bool) {
        let Some(cur) = self.focus else { return };
        let siblings = self.regions.siblings_of(cur);
        let target = if start {
            siblings.first()
        } else {
            siblings.last()
        };
        if let Some(&r) = target {
            self.focus = Some(r);
        }
    }

    /// 目前管理員分頁的清單長度。用來夾住選取範圍 ——
    /// 沒有上界的話往下按到底會一直累加，框框就消失了。
    pub fn admin_list_len(&self) -> usize {
        let key = match self.admin.tab() {
            AdminTab::Storage | AdminTab::Memory | AdminTab::Gpu | AdminTab::Network => "users",
        };
        self.admin
            .current()
            .ready()
            .and_then(|v| v.get(key))
            .and_then(|u| u.as_array())
            .map_or(0, |a| a.len())
    }

    /// 大幅捲動。
    fn page_scroll(&mut self, delta: i64) {
        if self.scroll_focused(delta) == Some(true) {
            self.focus_list_end(delta > 0);
        }
    }

    fn change_interval(&mut self, delta: f64) {
        let secs = (self.interval.as_secs_f64() + delta).clamp(0.2, 10.0);
        self.interval = Duration::from_secs_f64(secs);
        self.state.intervals = Intervals::from_base(self.interval);
        self.note(format!("更新間隔 {secs:.1}s"));
    }

    fn selected_storage_user(&self) -> Option<String> {
        let v = self.admin.storage.ready()?;
        let users = v.get("users")?.as_array()?;
        users
            .get(self.admin.selected)?
            .get("user")?
            .as_str()
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::describe::DescribeTarget;

    /// 測試用的 App：已經把啟動畫面關掉。
    ///
    /// 啟動畫面是 modal，第一個鍵只用來關它（不再穿透到頁面），
    /// 所以按鍵測試必須先過這一關 —— 否則每個測試的第一個 assert
    /// 量到的都是「splash 還開著」。
    fn app() -> App {
        let mut a = App::new(Config::default(), ColorDepth::Monochrome);
        a.modal = Modal::None;
        a
    }

    #[test]
    fn view_parsing_accepts_aliases() {
        assert_eq!(View::parse("mem"), Some(View::Memory));
        assert_eq!(View::parse("GPU"), Some(View::Gpu));
        assert_eq!(View::parse("disk"), Some(View::Storage));
        assert_eq!(View::parse("nonsense"), None);
    }

    #[test]
    fn number_keys_switch_pages() {
        let mut a = app();
        a.on_key(Key::Char('3'));
        assert_eq!(a.view, View::Gpu);
        a.on_key(Key::Char('6'));
        assert_eq!(a.view, View::Process);
        a.on_key(Key::Char('`'));
        assert_eq!(a.view, View::Overview);
    }

    #[test]
    fn esc_walks_back_up_one_level_at_a_time() {
        let mut a = app();
        a.on_key(Key::Char('2')); // 記憶體頁
        a.regions.clear();
        let p = a
            .regions
            .add_panel(ratatui::layout::Rect::new(0, 0, 40, 10), "mem.used", "RAM");
        a.regions.add_item(
            p,
            ratatui::layout::Rect::new(1, 1, 38, 1),
            "mem.cached",
            "cache",
        );

        a.on_key(Key::Enter); // → 面板
        a.on_key(Key::Enter); // → 列
        assert_eq!(a.focused_region().unwrap().label, "cache");
        a.on_key(Key::Esc);
        assert_eq!(a.focused_region().unwrap().label, "RAM", "Esc 應退回上一層");
        a.on_key(Key::Esc);
        assert!(a.focused_region().is_none(), "再 Esc 應回到分頁層");
        assert!(!a.quit);
        a.on_key(Key::Esc);
        assert_eq!(a.view, View::Overview, "分頁層再 Esc 才回總覽");
        assert!(!a.quit);
    }

    #[test]
    fn q_quits_from_anywhere() {
        let mut a = app();
        a.on_key(Key::Char('3'));
        assert_eq!(a.on_key(Key::Char('q')), SideEffect::Quit);
        assert!(a.quit);
    }

    #[test]
    fn tab_cycles_through_all_views() {
        let mut a = app();
        for _ in 0..VIEWS.len() {
            a.on_key(Key::Tab);
        }
        assert_eq!(a.view, View::Overview, "循環一輪應回到起點");
    }

    #[test]
    fn focus_follows_registered_regions_not_a_fixed_list() {
        // 空間焦點的核心：可選的東西來自「這一幀實際畫了什麼」，
        // 不是一份寫死的、跟畫面對不上的名詞清單。
        let a = app();
        a.regions.clear();
        a.regions
            .add_panel(ratatui::layout::Rect::new(0, 0, 40, 10), "cpu.usage", "CPU");
        a.regions.add_panel(
            ratatui::layout::Rect::new(40, 0, 40, 10),
            "mem.used",
            "Memory",
        );
        // 預設停在分頁層，還沒進入頁面
        assert_eq!(a.focus_position(), (0, 0));
        assert!(a.focused_region().is_none());
    }

    #[test]
    fn arrow_keys_move_between_siblings_after_entering_a_page() {
        let mut a = app();
        a.regions.clear();
        a.regions
            .add_panel(ratatui::layout::Rect::new(0, 0, 40, 10), "cpu.usage", "CPU");
        a.regions.add_panel(
            ratatui::layout::Rect::new(40, 0, 40, 10),
            "mem.used",
            "Memory",
        );
        a.regions
            .add_panel(ratatui::layout::Rect::new(0, 10, 40, 10), "gpu.util", "GPU");

        // 分頁層：←→ 是換頁，不是移動焦點
        assert!(a.focused_region().is_none());
        a.on_key(Key::Enter); // 進入這一頁
        assert_eq!(a.focused_region().unwrap().label, "CPU");

        a.on_key(Key::Right);
        assert_eq!(
            a.focused_region().unwrap().label,
            "Memory",
            "→ 應移到右邊那塊"
        );
        a.on_key(Key::Left);
        assert_eq!(a.focused_region().unwrap().label, "CPU");
        a.on_key(Key::Down);
        assert_eq!(a.focused_region().unwrap().label, "GPU", "↓ 應移到下面那塊");
    }

    #[test]
    fn explain_describes_whatever_is_boxed() {
        let mut a = app();
        a.regions.clear();
        a.regions
            .add_panel(ratatui::layout::Rect::new(0, 0, 40, 10), "cpu.usage", "CPU");
        a.regions.add_panel(
            ratatui::layout::Rect::new(40, 0, 40, 10),
            "mem.used",
            "Memory",
        );
        a.on_key(Key::Enter);
        a.on_key(Key::Right);
        a.on_key(Key::Char('e'));
        match a.modal {
            Modal::Explain { target } => assert_eq!(target, DescribeTarget::Metric("mem.used")),
            other => panic!("e 應解釋目前框住的那一塊，得到 {other:?}"),
        }
    }

    #[test]
    fn descending_reaches_individual_rows_for_finer_explanations() {
        // 使用者回報：只到面板層的話，Logical CPUs / Core clock 這些選不到，
        // 說明反而比寫死的清單還粗。
        let mut a = app();
        a.regions.clear();
        let p = a.regions.add_panel(
            ratatui::layout::Rect::new(0, 0, 40, 10),
            "cpu.load",
            "Core Statistics",
        );
        a.regions.add_item(
            p,
            ratatui::layout::Rect::new(1, 1, 38, 1),
            "cpu.usage",
            "Logical CPUs",
        );
        a.regions.add_item(
            p,
            ratatui::layout::Rect::new(1, 2, 38, 1),
            "cpu.freq",
            "Avg Frequency",
        );

        a.on_key(Key::Enter); // 進面板
        assert_eq!(a.focused_region().unwrap().label, "Core Statistics");
        a.on_key(Key::Enter); // 進列
        assert_eq!(a.focused_region().unwrap().label, "Logical CPUs");
        a.on_key(Key::Down);
        assert_eq!(a.focused_region().unwrap().label, "Avg Frequency");
        a.on_key(Key::Char('e'));
        match a.modal {
            Modal::Explain { target } => assert_eq!(target, DescribeTarget::Metric("cpu.freq")),
            other => panic!("應解釋選到的那一列，得到 {other:?}"),
        }
    }

    #[test]
    fn depth_is_not_capped() {
        let mut a = app();
        a.regions.clear();
        let l0 = a
            .regions
            .add_panel(ratatui::layout::Rect::new(0, 0, 40, 20), "proc.cpu", "L0");
        let l1 = a.regions.add_child(
            l0,
            ratatui::layout::Rect::new(1, 1, 38, 1),
            "proc.cpu",
            "L1",
        );
        let l2 = a.regions.add_child(
            l1,
            ratatui::layout::Rect::new(2, 2, 36, 1),
            "proc.threads",
            "L2",
        );
        a.regions.add_child(
            l2,
            ratatui::layout::Rect::new(3, 3, 34, 1),
            "proc.state",
            "L3",
        );
        for expected in ["L0", "L1", "L2", "L3"] {
            a.on_key(Key::Enter);
            assert_eq!(a.focused_region().unwrap().label, expected);
        }
        assert_eq!(a.focus_depth(), 4, "層數不該被限制");
    }

    #[test]
    fn admin_selection_is_clamped_to_the_list() {
        // 使用者回報：往下按到底還會繼續跑，框框消失，要按同樣次數的 ↑ 才回來
        let mut a = app();
        a.view = View::Admin;
        a.admin.storage = Async::Ready(serde_json::json!({
            "users": [{"user":"a"},{"user":"b"},{"user":"c"}]
        }));
        assert_eq!(a.admin_list_len(), 3);
        for _ in 0..20 {
            a.scroll_focused(1);
        }
        assert_eq!(a.admin.selected, 2, "不可以超出清單長度");
        a.scroll_focused(-1);
        assert_eq!(a.admin.selected, 1, "往上一次就要立刻回應");
    }

    #[test]
    fn admin_selection_wraps_at_both_ends() {
        // 使用者回報：到底再按 ↓，箭頭跳到第一個但反白沒跟上；按住不放
        // 會自己轉圈，放開才真的選到 —— 焦點與選取各走各的。現在繞的是
        // 選取本身。
        let mut a = app();
        a.view = View::Admin;
        a.admin.storage = Async::Ready(serde_json::json!({
            "users": [{"user":"a"},{"user":"b"},{"user":"c"}]
        }));
        a.admin.selected = 2;
        assert_eq!(a.scroll_focused(1), Some(true), "到底再按 ↓ 要繞");
        assert_eq!(a.admin.selected, 0);
        assert_eq!(a.scroll_focused(-1), Some(true), "到頂再按 ↑ 要繞");
        assert_eq!(a.admin.selected, 2);
        assert_eq!(a.scroll_focused(-1), Some(false));
        assert_eq!(a.admin.selected, 1);
        // PageDown 走到底就停在最後一列，再按一次才繞
        a.page_scroll(10);
        assert_eq!(a.admin.selected, 2);
        a.page_scroll(10);
        assert_eq!(a.admin.selected, 0);
        // 只有一列：沒東西可繞
        a.admin.storage = Async::Ready(serde_json::json!({ "users": [{"user":"a"}] }));
        a.admin.selected = 0;
        assert_eq!(a.scroll_focused(1), None);
    }

    #[test]
    fn arrows_scroll_when_walking_off_the_end_of_a_list() {
        // 清單裡的列走到底時改成捲動，而不是繞回第一列
        let mut a = app();
        a.view = View::Process;
        a.regions.clear();
        let p = a.regions.add_list_panel(
            ratatui::layout::Rect::new(0, 0, 80, 30),
            "proc.cpu",
            "Processes",
        );
        a.regions.add_item(
            p,
            ratatui::layout::Rect::new(1, 1, 78, 1),
            "proc.cpu",
            "row0",
        );
        a.regions.add_item(
            p,
            ratatui::layout::Rect::new(1, 2, 78, 1),
            "proc.cpu",
            "row1",
        );

        a.on_key(Key::Enter); // 進面板
        a.on_key(Key::Enter); // 進清單的第一列
        assert_eq!(a.focused_region().unwrap().label, "row0");
        a.on_key(Key::Down);
        assert_eq!(a.focused_region().unwrap().label, "row1");

        // 清單底下還有東西沒顯示（畫的人回報上界）→ 走到底就捲動
        a.set_scroll_bound(5);
        let before = a.scroll;
        a.on_key(Key::Down);
        assert_eq!(a.scroll, before + 1, "還捲得動卻沒有捲");

        // 捲到底了 → 繞到開頭：捲動位置歸零、焦點框到第一列，**兩個一起**。
        // 早期是停住不動但焦點框自己繞：箭頭在第一列、選取還在最後一列。
        a.set_scroll_bound(a.scroll);
        assert_eq!(a.focused_region().unwrap().label, "row1");
        a.on_key(Key::Down);
        assert_eq!(a.scroll, 0, "捲到底再按 ↓ 應該繞回開頭");
        assert_eq!(
            a.focused_region().unwrap().label,
            "row0",
            "繞回開頭時焦點框要跟著到第一列"
        );
        // 開頭往上：繞到最底，焦點框到最後一列
        a.set_scroll_bound(5);
        a.on_key(Key::Up);
        assert_eq!(a.scroll, 5, "開頭再按 ↑ 應該繞到最底");
        assert_eq!(a.focused_region().unwrap().label, "row1");
        // 而且永遠不會累加到看不見的地方
        for _ in 0..40 {
            a.on_key(Key::Down);
        }
        assert!(a.scroll <= 5, "捲動位置跑出上界：{}", a.scroll);
    }

    #[test]
    fn scrollable_metrics_are_declared() {
        assert!(is_scrollable_metric("proc.cpu"));
        assert!(is_scrollable_metric("storage.user"));
        assert!(!is_scrollable_metric("cpu.usage"));
    }

    #[test]
    fn vertical_moves_wrap_but_horizontal_ones_do_not() {
        let mut a = app();
        a.regions.clear();
        a.regions
            .add_panel(ratatui::layout::Rect::new(0, 0, 40, 10), "cpu.usage", "CPU");
        a.regions
            .add_panel(ratatui::layout::Rect::new(0, 10, 40, 10), "gpu.util", "GPU");
        a.on_key(Key::Enter);
        a.on_key(Key::Up); // 最上面往上：繞到最後一個
        assert_eq!(
            a.focused_region().unwrap().label,
            "GPU",
            "↑↓ 到邊界應繞回去"
        );
        a.on_key(Key::Down);
        assert_eq!(a.focused_region().unwrap().label, "CPU");
        // 左右到底就停著，不繞 —— 繞了會讓人以為跳到別的地方
        a.on_key(Key::Left);
        assert_eq!(a.focused_region().unwrap().label, "CPU", "← 到底應該停著");
    }

    #[test]
    fn arrows_switch_pages_only_at_the_page_level() {
        let mut a = app();
        a.regions.clear();
        a.regions
            .add_panel(ratatui::layout::Rect::new(0, 0, 40, 10), "cpu.usage", "CPU");
        a.regions.add_panel(
            ratatui::layout::Rect::new(40, 0, 40, 10),
            "mem.used",
            "Memory",
        );

        // 分頁層：← → 換頁
        let v = a.view;
        a.on_key(Key::Right);
        assert_ne!(a.view, v, "分頁層的 → 應該換頁");

        // 進入頁面之後：← → 是在面板之間移動，不再換頁
        a.regions.clear();
        a.regions
            .add_panel(ratatui::layout::Rect::new(0, 0, 40, 10), "cpu.usage", "CPU");
        a.regions.add_panel(
            ratatui::layout::Rect::new(40, 0, 40, 10),
            "mem.used",
            "Memory",
        );
        a.on_key(Key::Enter);
        let v2 = a.view;
        a.on_key(Key::Right);
        assert_eq!(a.view, v2, "進入頁面後 → 不該換頁");
        assert_eq!(a.focused_region().unwrap().label, "Memory");

        // Tab 在任何一層都能換頁
        a.on_key(Key::Tab);
        assert_ne!(a.view, v2);
    }

    #[test]
    fn any_key_closes_a_modal() {
        let mut a = app();
        a.modal = Modal::Help;
        a.on_key(Key::Char('x'));
        assert_eq!(a.modal, Modal::None);
    }

    #[test]
    fn filter_mode_captures_text_keys() {
        let mut a = app();
        a.on_key(Key::Char('6'));
        a.on_key(Key::Char('/'));
        assert!(a.filter_editing);
        for c in "rsync".chars() {
            a.on_key(Key::Char(c));
        }
        assert_eq!(a.filter, "rsync");
        // 在篩選模式下 'q' 是文字不是離開
        a.on_key(Key::Char('q'));
        assert_eq!(a.filter, "rsyncq");
        assert!(!a.quit);
        a.on_key(Key::Enter);
        assert!(!a.filter_editing);
    }

    #[test]
    fn esc_clears_filter() {
        let mut a = app();
        a.on_key(Key::Char('6'));
        a.on_key(Key::Char('/'));
        a.on_key(Key::Char('x'));
        a.on_key(Key::Esc);
        assert!(a.filter.is_empty());
        assert!(!a.filter_editing);
    }

    #[test]
    fn interval_is_clamped_to_safe_range() {
        let mut a = app();
        for _ in 0..50 {
            a.on_key(Key::Char('+'));
        }
        assert!(
            a.interval.as_secs_f64() >= 0.2,
            "不可讓使用者把取樣頻率調到危險等級"
        );
        for _ in 0..100 {
            a.on_key(Key::Char('-'));
        }
        assert!(a.interval.as_secs_f64() <= 10.0);
    }

    #[test]
    fn confirmation_defaults_to_cancel() {
        let c = Confirmation::signal(1234, 999, "alice", "python train.py", SafeSignal::Term);
        assert!(!c.confirmed_selected, "危險操作的預設選項必須是取消");
    }

    #[test]
    fn confirmation_shows_the_actual_target() {
        let c = Confirmation::signal(1234, 999, "alice", "python train.py", SafeSignal::Term);
        assert_eq!(c.target_pid, 1234);
        assert_eq!(c.target_user, "alice");
        assert!(
            c.target_command.contains("train.py"),
            "必須顯示實際會被影響的指令"
        );
    }

    #[test]
    fn confirmation_warns_about_root_targets() {
        let c = Confirmation::signal(1, 1, "root", "systemd", SafeSignal::Term);
        assert!(c.warning.is_some(), "對 root 行程動手應該有額外警告");
    }

    #[test]
    fn enter_on_cancel_does_not_execute() {
        let mut a = app();
        let c = Confirmation::signal(1234, 999, "alice", "x", SafeSignal::Term);
        a.modal = Modal::Confirm(c);
        a.on_key(Key::Enter); // 停在「取消」上
        assert_eq!(a.modal, Modal::None);
        assert_eq!(a.status_text(), Some("已取消"));
    }

    #[test]
    fn confirmation_requires_moving_to_confirm_first() {
        let mut a = app();
        let c = Confirmation::signal(1234, 999, "alice", "x", SafeSignal::Term);
        a.modal = Modal::Confirm(c);
        a.on_key(Key::Right);
        match &a.modal {
            Modal::Confirm(c) => assert!(c.confirmed_selected),
            _ => panic!("應仍停留在確認視窗"),
        }
    }

    #[test]
    fn admin_query_attempts_are_safe_without_privilege() {
        // 這裡刻意**不**斷言「沒授權就不要送出查詢」——
        // 那正是原本的 bug：用快取狀態擋住呼叫，導致憑證明明有效
        // 也永遠查不到東西。有權決定的是 sudo，不是我們的快取。
        //
        // 真正該保證的是：查詢可以嘗試，但一定 fail closed，
        // 而且因為用的是 sudo -n，絕不會在 TUI 上跳出密碼提示。
        let mut a = app();
        a.view = View::Admin;
        a.request_admin(true);
        // 等背景 worker 回來
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(50));
            a.tick(Instant::now());
            if !a.admin.storage.is_running() {
                break;
            }
        }
        match a.admin.current() {
            Async::Ready(v) => {
                // 真的有授權（sudo 快取還在）：資料必須是結構化的
                assert!(v["users"].is_array(), "成功時必須回傳結構化資料");
            }
            Async::Failed(e) => {
                assert!(!e.is_empty(), "失敗必須有明確原因");
                // 絕不可在失敗訊息裡洩漏任何使用者資料
                assert!(!e.contains("/home/"), "錯誤訊息不可含家目錄路徑");
            }
            Async::Running { .. } | Async::Idle => {
                // helper 沒安裝時會維持 Idle，這也是合法的 fail closed
            }
        }
    }

    #[test]
    fn u_key_requests_sudo_when_locked() {
        let mut a = app();
        if matches!(a.privilege.state(), PrivilegeState::Locked) {
            assert_eq!(a.on_key(Key::Char('u')), SideEffect::AuthenticateSudo);
        }
    }

    #[test]
    fn u_when_already_unlocked_actually_does_something() {
        // 以前按 u 只印一行「已解鎖」，畫面完全沒變 —— 使用者會以為壞掉了。
        // 現在要真的帶去管理員頁並開始查詢。
        let mut a = app();
        if !a.privilege.state().is_available() {
            // 測試環境沒有 sudo 快取，直接驗證邏輯分支即可
            return;
        }
        a.on_key(Key::Char('u'));
        assert_eq!(a.view, View::Admin, "已解鎖時按 u 應帶去管理員頁");
        assert!(a.status_text().is_some());
    }

    #[test]
    fn theme_cycling_stays_valid() {
        let mut a = app();
        for _ in 0..crate::theme::THEMES.len() * 2 {
            a.on_key(Key::Char('t'));
            assert!(crate::theme::THEMES.contains(&a.theme.name));
        }
    }

    #[test]
    fn every_view_has_explainable_metrics() {
        for v in VIEWS {
            assert!(
                !v.metrics().is_empty(),
                "{} 頁沒有可解釋的 metric",
                v.title()
            );
            assert!(!v.title().is_empty());
            assert!(!v.key().is_empty());
        }
    }

    #[test]
    fn pause_toggles_and_reports() {
        let mut a = app();
        a.on_key(Key::Char(' '));
        assert!(a.paused);
        assert!(a.status_text().is_some());
        a.on_key(Key::Char(' '));
        assert!(!a.paused);
    }

    #[test]
    fn a_startup_advisory_outlives_ordinary_notes_but_yields_to_them() {
        // 啟動提醒（例如私人版遮住系統版）要留得夠久讓人看到；
        // 一般狀態訊息出現時先讓它，四秒後提醒還在。
        let mut a = app();
        a.advise("私人版遮住系統版");
        assert_eq!(a.status_text(), Some("私人版遮住系統版"));
        a.note("主題：nord");
        assert_eq!(a.status_text(), Some("主題：nord"), "一般訊息優先");
        a.status = Some(("主題：nord".into(), Instant::now() - Duration::from_secs(5)));
        assert_eq!(
            a.status_text(),
            Some("私人版遮住系統版"),
            "訊息過期後提醒還在"
        );
        a.advisory = Some((
            "私人版遮住系統版".into(),
            Instant::now() - Duration::from_secs(21),
        ));
        assert_eq!(a.status_text(), None, "二十秒後提醒也該收起來");
    }

    // ── 彩蛋排行榜 ──────────────────────────────────────────────────

    fn scratch_store(tag: &str) -> crate::scoreboard::Store {
        let dir =
            std::env::temp_dir().join(format!("sysview-app-scores-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let uid = crate::collectors::util::real_uid();
        crate::scoreboard::Store::with_paths(
            Some(dir.clone()),
            dir.join(format!("{uid}.json")),
            uid,
        )
    }

    /// 開一局固定種子的遊戲，什麼都不按，跑到死。回傳分數。
    fn play_until_dead(a: &mut App) -> u32 {
        use crate::ui::dino::{Dino, Rng, FRAME};
        let mut t = Instant::now();
        a.dino = Some(Dino::with_rng(t, 0, Rng::from_seed(7)));
        a.modal = Modal::Dino;
        a.dino_over = None;
        for _ in 0..(20.0 / FRAME.as_secs_f32()) as usize {
            t += FRAME;
            a.tick_game(t);
            if a.dino.as_ref().is_some_and(|g| g.is_over()) {
                break;
            }
        }
        let g = a.dino.as_ref().unwrap();
        assert!(g.is_over(), "20 秒都沒死");
        g.score()
    }

    #[test]
    fn dying_with_a_qualifying_score_asks_for_a_name_and_records_it() {
        let mut a = app();
        a.scores = scratch_store("record");
        let score = play_until_dead(&mut a);
        assert!(score > 0);
        assert_eq!(
            a.dino_over.as_ref().and_then(|o| o.typed.as_deref()),
            Some(""),
            "空榜：任何分數都要問名字"
        );
        // 輸入框開著時 r 是字母，不是重來
        a.on_key(Key::Char('r'));
        assert!(a.dino.as_ref().unwrap().is_over());
        a.on_key(Key::Backspace);
        for c in "Ada".chars() {
            a.on_key(Key::Char(c));
        }
        assert_eq!(a.dino_over.as_ref().unwrap().typed.as_deref(), Some("Ada"));
        a.on_key(Key::Enter);
        let over = a.dino_over.as_ref().unwrap();
        assert!(over.typed.is_none());
        assert!(
            over.note.as_deref().unwrap_or("").starts_with("已記錄"),
            "{:?}",
            over.note
        );
        assert_eq!(over.board.top()[0].name, "Ada");
        assert_eq!(a.scores.load().top()[0].score, score);
        // 這時 r 才是重來；重來後上一局的榜與訊息清掉
        a.on_key(Key::Char('r'));
        assert!(!a.dino.as_ref().unwrap().is_over());
        assert!(a.dino_over.is_none());
    }

    #[test]
    fn skipping_or_leaving_the_name_empty_records_nothing() {
        let mut a = app();
        a.scores = scratch_store("skip");
        play_until_dead(&mut a);
        a.on_key(Key::Enter); // 空的：當沒按
        assert!(a.dino_over.as_ref().unwrap().typed.is_some());
        a.on_key(Key::Char(' '));
        a.on_key(Key::Enter); // 只有空白：也當沒按
        assert!(a.dino_over.as_ref().unwrap().typed.is_some());
        a.on_key(Key::Esc);
        let over = a.dino_over.as_ref().unwrap();
        assert!(over.typed.is_none());
        assert_eq!(over.note.as_deref(), Some("沒有記錄"));
        assert!(a.scores.load().is_empty(), "不填就不記");
        assert!(
            matches!(a.modal, Modal::Dino),
            "輸入框裡的 Esc 只是不記錄，不關視窗"
        );
        a.on_key(Key::Esc);
        assert!(matches!(a.modal, Modal::None));
        assert!(a.dino_over.is_none());
    }

    #[test]
    fn a_score_that_does_not_beat_fifth_place_is_not_asked_about() {
        let mut a = app();
        a.scores = scratch_store("full");
        for i in 0..5 {
            a.scores.record(&format!("u{i}"), 100_000 + i).unwrap();
        }
        play_until_dead(&mut a);
        let over = a.dino_over.as_ref().unwrap();
        assert!(over.typed.is_none(), "沒有嚴格高於第五名，不問");
        assert_eq!(over.board.top().len(), 5);
        // 死掉之後 r 直接重來
        a.on_key(Key::Char('r'));
        assert!(!a.dino.as_ref().unwrap().is_over());
    }

    #[test]
    fn the_same_name_with_a_lower_score_is_refused_with_an_explanation() {
        let mut a = app();
        a.scores = scratch_store("lower");
        a.scores.record("Ada", 100_000).unwrap();
        play_until_dead(&mut a);
        for c in "Ada".chars() {
            a.on_key(Key::Char(c));
        }
        a.on_key(Key::Enter);
        let over = a.dino_over.as_ref().unwrap();
        assert!(over.note.as_deref().unwrap().contains("沒有更高"));
        assert_eq!(a.scores.load().best_of("Ada"), Some(100_000));
    }

    #[test]
    fn the_name_box_is_capped_at_its_display_width() {
        let mut a = app();
        a.scores = scratch_store("cap");
        play_until_dead(&mut a);
        for _ in 0..30 {
            a.on_key(Key::Char('測'));
        }
        let typed = a.dino_over.as_ref().unwrap().typed.clone().unwrap();
        assert_eq!(
            crate::ui::format::width(&typed),
            crate::scoreboard::NAME_WIDTH
        );
    }
}
