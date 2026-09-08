//! sysview 進入點。
//!
//! 負責三件事：解析命令列、管理終端機生命週期、跑事件迴圈。
//! 業務邏輯全部在 [`sysview`] library 裡。

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use sysview::app::{App, Key, SideEffect};
use sysview::config::Config;
use sysview::theme::ColorDepth;

#[derive(Parser, Debug)]
#[command(
    name = "sysview",
    // 版本後面帶 git commit：使用者回報問題時才分得出手上跑的是哪一版
    // （build.rs 提供；沒有 git 的環境是 "unknown"）
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("SYSVIEW_GIT_REV"), ")"),
    about = "Linux 系統觀測工具：CPU / 記憶體 / GPU / 儲存 / 網路 / 行程",
    long_about = "sysview 把 Linux 的系統觀測、教學、metric 來源追溯、診斷，\n\
                  以及安全的管理員深入資訊整合在同一個 TUI。\n\n\
                  按 e 可以看到任何數字的意義、來源、算式、陷阱與對應的原生指令。"
)]
struct Cli {
    /// 更新間隔（秒），0.2–60
    #[arg(short, long)]
    interval: Option<f64>,

    /// 直接開在指定頁面
    #[arg(short = 'v', long, value_name = "PAGE")]
    view: Option<String>,

    /// 主題名稱
    #[arg(long)]
    theme: Option<String>,

    /// 印出純文字快照後離開（無 TTY 時自動採用）
    #[arg(short = 's', long, alias = "once")]
    snapshot: bool,

    /// 印出 JSON 後離開，適合 `sysview --json | jq`
    #[arg(long)]
    json: bool,

    /// 列出所有可解釋的 metric
    #[arg(long)]
    list_metrics: bool,

    /// 解釋單一 metric 後離開（不進入 TUI）
    #[arg(long, value_name = "METRIC_ID")]
    explain: Option<String>,

    /// 印出預設設定檔範本
    #[arg(long)]
    print_config: bool,
    /// 印出登入用的歡迎畫面（logo、吉祥物、一句話）後離開；給 `make install-motd` 用
    #[arg(long)]
    banner: bool,

    /// 停用顏色（也可用 NO_COLOR 環境變數）
    #[arg(long)]
    no_color: bool,

    /// 停用 GPU 監控。會跳過 NVML 的 dlopen，常駐記憶體少約 15 MB
    /// （那 15 MB 是 NVIDIA 驅動函式庫自己的，不是 sysview 佔用的）
    #[arg(long)]
    no_gpu: bool,

    /// 強制顯示啟動品牌畫面（1.5 秒後自己消失，按任意鍵也可跳過）
    #[arg(long)]
    splash: bool,

    /// 跳過啟動品牌畫面
    #[arg(long)]
    no_splash: bool,

    /// 指定吉祥物：fox / deer / none
    #[arg(long, value_name = "WHICH")]
    mascot: Option<String>,
}

fn main() -> std::process::ExitCode {
    // 放進管線是這支程式的常見用法（`--json | jq`、`--snapshot | head`）。
    // 讀端提早離開時要安靜結束，不是 panic 出一行 Broken pipe。
    sysview::collectors::util::restore_default_sigpipe();

    let cli = Cli::parse();

    if cli.print_config {
        print!("{}", Config::template());
        return std::process::ExitCode::SUCCESS;
    }
    if cli.list_metrics {
        list_metrics();
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(id) = &cli.explain {
        return explain_metric(id);
    }

    let (mut config, warning) = Config::load();
    if cli.banner {
        // 登入畫面是安裝時產成檔案的：這裡沒有 TTY、沒有 collector、沒有特權
        let brand =
            sysview::ui::visual::logo::Brand::new(&config.branding.name, &config.branding.logo);
        let colour = !cli.no_color && std::env::var_os("NO_COLOR").is_none();
        print!("{}", sysview::banner::banner(&brand, colour));
        return std::process::ExitCode::SUCCESS;
    }
    if let Some(i) = cli.interval {
        config.interval = i;
    }
    if let Some(t) = &cli.theme {
        config.theme = t.clone();
    }
    if cli.no_gpu {
        config.gpu = false;
    }
    if cli.splash {
        config.branding.splash = true;
    }
    if cli.no_splash {
        config.branding.splash = false;
    }
    if let Some(m) = &cli.mascot {
        config.branding.mascot = m.clone();
    }
    if let Some(v) = &cli.view {
        if sysview::app::View::parse(v).is_none() {
            eprintln!("錯誤：未知頁面 {v:?}（可用：overview cpu memory gpu storage network process admin）");
            return std::process::ExitCode::from(2);
        }
        config.default_page = v.clone();
    }
    let (config, clamp_note) = config.sanitized();

    // 沒有 TTY 就自動切成非互動模式 —— 這樣 pipe / cron / watch 都能用
    let interactive = std::io::stdout().is_terminal() && !cli.snapshot && !cli.json;
    if !interactive {
        let interval = config.interval_duration().min(Duration::from_secs(2));
        if cli.json {
            let v = sysview::snapshot::json(config.history, interval, config.gpu);
            match serde_json::to_string_pretty(&v) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("JSON 序列化失敗：{e}");
                    return std::process::ExitCode::FAILURE;
                }
            }
        } else {
            print!(
                "{}",
                sysview::snapshot::text(config.history, interval, config.gpu)
            );
        }
        return std::process::ExitCode::SUCCESS;
    }

    let depth = if cli.no_color {
        ColorDepth::Monochrome
    } else {
        ColorDepth::detect()
    };

    // 以 root 跑整個 TUI 是不建議的做法，明講但不阻擋
    if sysview::privilege::running_as_root() {
        eprintln!("提醒：{}", sysview::privilege::ROOT_WARNING);
        eprintln!("（3 秒後繼續）");
        std::thread::sleep(Duration::from_secs(3));
    }

    let terminal = match ratatui::try_init() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("無法初始化終端機：{e}");
            eprintln!("（試著放大視窗，或確認 TERM 設定正確）");
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut app = App::new(config, depth);
    if let Some(w) = warning.or(clamp_note) {
        app.note(w);
    }
    if let Some(w) = shadowed_by_private_copy() {
        app.advise(w);
    }

    let result = run(terminal, &mut app);
    ratatui::restore();

    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sysview 結束於錯誤：{e}");
            std::process::ExitCode::FAILURE
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 事件迴圈的三條不變量
//
// 寫在這裡是因為它們很容易在後續改動中被無意破壞 ——
// 尤其是「動畫的 FPS」看起來像一個跟輸入同等重要的排程需求，其實不是。
//
// ── 1. 互動延遲 ───────────────────────────────────────────────────
//
//   有待處理的互動輸入時，不開始任何純裝飾工作。
//   已到期但還沒呈現的裝飾格可以直接丟掉。
//   輸入與狀態更新完之後，只需要畫出**最新**的狀態。
//
//   實作：輸入在迴圈最頂端一次全部消化完；要畫之前再確認一次沒有新輸入；
//   輸入造成的重畫不受畫格節流限制（[`should_draw`]）。
//
// ── 2. 動畫不累積欠幀 ─────────────────────────────────────────────
//
//   動畫不補幀。落後了就直接跳到現在該有的樣子，不重播中間那些。
//
//   實作：[`App::tick_animation`] 每次呼叫只前進一格，而且把時間基準
//   設成 `now` 而不是 `last + period`。停了五秒再回來也只會前進一格。
//
// ── 3. 重畫合併 ───────────────────────────────────────────────────
//
//   同一輪裡的多種狀態變化只畫一次。
//
//   實作：一次迴圈最多一次 `terminal.draw`。取樣、時鐘、動畫、輸入
//   各自只是把 dirty 旗標立起來，畫的決定集中在一個地方。
//
// 另外兩條相關的：
//
//   * 動畫時鐘與採樣時鐘完全分開。動畫再怎麼跑都不會讓 collector
//     多讀一次 `/proc`、多 poll 一次 NVML（有測試在守）。
//   * 畫面上沒有吉祥物時，動畫時鐘整個停掉 —— 八個頁面裡只有總覽有
//     裝飾槽，小終端機更是完全放不下。
// ═══════════════════════════════════════════════════════════════════════

/// 這一輪該不該重畫。
///
/// 分成兩種來源是刻意的：
///
/// * `input` —— 使用者做了什麼。**立刻畫**，不受畫格節流限制。
///   事件在畫之前已經全部消化完，所以不受節流也不會失控重畫。
/// * `background` —— 取樣、頁首時鐘、吉祥物動畫。受節流限制，
///   免得一個閒置的監控工具持續燒 CPU。
///
/// 兩者混在一起的話，按鍵會被為了動畫設的 25fps 上限擋住最多 40 毫秒 ——
/// 裝飾不該讓操作等。
/// 輸出端塞住時自動降幀。
///
/// `terminal.draw` 裡的 flush 是阻塞的：終端機（或 ssh）消化不了，write 就
/// 卡在那裡，主迴圈連輸入都讀不到 —— 使用者按了跳，恐龍要等終端機追上
/// 才跳。所以量每一格 draw（含 flush）花了多久：超過一格的時間就是被塞住
/// 了，接下來一秒把畫格拉長一倍（60 → 30 fps，正好一半，跟螢幕更新率仍然
/// 對齊），送一半的位元組讓它追上來；追上了就恢復。
/// 只在遊戲開著時有作用 —— 儀表板本來就在 25 fps 以下。
struct Pacer {
    slow_until: Option<Instant>,
}

impl Pacer {
    /// 降幀維持多久。太短會在塞與不塞之間抖；一秒夠終端機把積壓的畫完。
    const HOLD: Duration = Duration::from_secs(1);

    fn new() -> Self {
        Self { slow_until: None }
    }

    /// 畫完一格回報：`took` 是這一格 draw（含 flush）花的時間。
    fn record(&mut self, drew_at: Instant, took: Duration, frame: Duration) {
        if took > frame {
            self.slow_until = Some(drew_at + Self::HOLD);
        }
    }

    /// 現在一格至少要多久。
    fn min_frame(&self, now: Instant, frame: Duration) -> Duration {
        if self.slow_until.is_some_and(|t| now < t) {
            frame * 2
        } else {
            frame
        }
    }
}

fn should_draw(input: bool, background: bool, since_draw: Duration, min_frame: Duration) -> bool {
    input || (background && since_draw >= min_frame)
}

/// 動畫該不該在這一輪推進。
///
/// 使用者剛操作過就完全讓位。沒有人會因為狐狸少眨一次眼而困擾，
/// 但會因為按鍵慢半拍而困擾。
fn animation_may_advance(since_input: Duration, settle: Duration) -> bool {
    since_input >= settle
}

/// 開始 / 結束一次 synchronized update。
///
/// 失敗不影響正確性 —— 沒有這對序列畫面一樣會更新，只是會看到漸進重畫，
/// 所以呼叫端一律忽略錯誤。
fn draw_sync(begin: bool) -> std::io::Result<()> {
    use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
    let mut out = std::io::stdout();
    if begin {
        crossterm::execute!(out, BeginSynchronizedUpdate)
    } else {
        crossterm::execute!(out, EndSynchronizedUpdate)
    }
}

fn run(mut terminal: DefaultTerminal, app: &mut App) -> std::io::Result<()> {
    // 畫面是**事件驅動**的：只有取樣到新資料、按了鍵、或視窗大小改變時才重畫。
    // 無條件以固定 fps 重畫會讓一個閒置的監控工具持續燒 CPU，
    // 那正是 sysview 不該做的事（它自己不能成為 server 的負載來源）。
    //
    // 唯一的例外是頁首時鐘，所以最慢也每秒醒來一次。
    //
    // # 輸入優先於一切
    //
    // 這個迴圈的順序是刻意的：**先把排隊的輸入吃完，再考慮要不要畫**，
    // 而且畫之前會再確認一次沒有新的輸入進來。
    //
    // 反過來寫（先畫再收輸入）的話，動畫剛好到期而使用者同時按了鍵時，
    // 那一格動畫會先畫完、連同終端機的寫入一起花掉，按鍵才被看到。
    // 沒有人會因為狐狸少眨一次眼而困擾，但會因為按鍵慢半拍而困擾。
    // 背景重畫的上限，避免閒置時燒 CPU。彩蛋開著時改用遊戲自己的節奏 ——
    // 25 fps 對儀表板綽綽有餘，但對一個橫向捲動的遊戲來說每格會跳掉
    // 畫布寬的 2.4%（Chrome 是 1%），看起來就是一頓一頓。
    let idle_frame = Duration::from_millis(40);
    let clock_tick = Duration::from_secs(1);
    // 操作結束後多久才讓動畫回來。按住方向鍵捲動時，狐狸整段時間都停著。
    let settle = Duration::from_millis(250);
    let mut last_draw = Instant::now() - clock_tick;
    let mut last_input = Instant::now() - settle;
    // 兩種「該重畫」分開記：
    //   input_dirty  使用者做了什麼 → 立刻畫，不受畫格節流限制
    //   bg_dirty     取樣 / 時鐘 / 動畫 → 受節流限制，避免閒置時燒 CPU
    // 混在一起的話，按鍵會被為了動畫而設的 25fps 上限擋住最多 40 毫秒。
    // 事件在畫之前已經全部消化完，所以不受節流也不會失控重畫。
    let mut input_dirty = true;
    let mut bg_dirty = false;
    let mut pacer = Pacer::new();

    loop {
        // ── 1. 輸入最優先：先把排隊的全部處理完 ─────────────────────
        //
        // 一次只處理一個的話，按住方向鍵時每個事件都要等一個畫格，
        // 游標會離手指越來越遠；而中間那些畫面沒人看得到，畫了也是白畫。
        let mut had_input = false;
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    // Ctrl-C 一律離開，不管在哪個狀態
                    if k.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(k.code, KeyCode::Char('c'))
                    {
                        return Ok(());
                    }
                    if let Some(key) = translate(k) {
                        input_dirty = true;
                        had_input = true;
                        match app.on_key(key) {
                            SideEffect::Quit => return Ok(()),
                            SideEffect::AuthenticateSudo => {
                                terminal = authenticate(terminal, app)?;
                                last_draw = Instant::now() - clock_tick;
                            }
                            SideEffect::None => {}
                        }
                    }
                }
                Event::Resize(_, _) => {
                    // 這裡刻意**不呼叫** `terminal.clear()`，理由跟 `authenticate()`
                    // 那邊一樣，而且更嚴重。
                    //
                    // `Terminal::clear()` 第一件事是 `get_cursor_position()`，
                    // 也就是送出 `ESC[6n` 然後**等終端機回答**，上限兩秒。
                    // 而 sysview 自己就是 stdin 唯一的讀取者，那個回覆常常
                    // 拿不到 —— 量到的結果是：每改一次視窗大小，接下來的
                    // 第一個按鍵要等 1.9 秒才有反應（p90 1889ms）。
                    // 那正是「操作體感偏慢」的真正來源，跟渲染速度無關。
                    //
                    // 而且它是多餘的：下一次 `draw()` 會呼叫 `autoresize()`，
                    // 它內部的 `resize()` 本來就會 `clear_viewport()`、
                    // 在寬度縮小時再 clear 一次整頁，而且全部不查游標。
                    last_draw = Instant::now() - clock_tick;
                    input_dirty = true;
                    had_input = true;
                }
                _ => {}
            }
        }
        if app.quit {
            return Ok(());
        }
        let now = Instant::now();
        if had_input {
            last_input = now;
        }
        let interacting = now.duration_since(last_input) < settle;

        // ── 2. 時鐘：取樣、頁首時鐘、動畫 ───────────────────────────
        if app.tick(now) {
            bg_dirty = true;
        }
        if now.duration_since(last_draw) >= clock_tick {
            bg_dirty = true;
        }
        // 動畫是裝飾，使用者在操作時它完全讓位 —— 一格都不推進。
        // 它也**不會**讓 collector 多取樣一次，兩個時鐘從頭到尾是分開的。
        if animation_may_advance(now.duration_since(last_input), settle) && app.tick_animation(now)
        {
            bg_dirty = true;
        }
        // 彩蛋遊戲。跟動畫不同，它**不**在操作後讓位 —— 遊戲本身就是操作，
        // 讓位只會讓恐龍在按跳的瞬間卡住。沒在玩的時候這個呼叫是一個
        // `Option` 判斷，而 `next_game_tick_in` 會回傳一小時，
        // 所以主迴圈一次都不會為它醒來。
        if app.tick_game(now) {
            bg_dirty = true;
        }

        // ── 3. 要畫之前再確認一次沒有新的輸入 ───────────────────────
        //
        // 剛剛那一輪之後可能又有鍵進來了。與其花掉一次完整重畫再回頭處理，
        // 不如直接回到迴圈頂端把它吃掉 —— 那一幀反正也要被下一幀蓋掉。
        // 使用者做的事立刻畫；背景更新才受畫格節流限制。
        let min_frame = if app.dino.is_some() {
            pacer.min_frame(now, sysview::ui::dino::FRAME)
        } else {
            idle_frame
        };
        let due = should_draw(
            input_dirty,
            bg_dirty,
            now.duration_since(last_draw),
            min_frame,
        );
        if due && !event::poll(Duration::ZERO)? {
            // Synchronized output（DEC 2026）：把一幀包起來，讓終端機收完
            // 整幀再換上去，而不是收到多少畫多少。
            //
            // 為什麼值得：一次換頁大約送出 16–21 KB（量過），沒有這個標記
            // 的終端機會邊收邊重畫，畫面是一條一條刷過去的。程式端的延遲
            // 其實只有 0.8 毫秒 —— 使用者看到的「慢」是那個漸進重畫。
            // 成本是每幀 12 個位元組，不支援的終端機會直接忽略這兩個序列。
            let drew_at = Instant::now();
            let _ = draw_sync(true);
            let r = terminal.draw(|f| sysview::ui::draw(app, f));
            let _ = draw_sync(false);
            r?;
            pacer.record(drew_at, drew_at.elapsed(), min_frame);
            // region 清單剛剛重建過，焦點框要重新對回選取的那一列
            app.realign_focus();
            last_draw = now;
            input_dirty = false;
            bg_dirty = false;
        }

        // ── 4. 睡到下一個期限，或直到有輸入 ─────────────────────────
        //
        // `until_frame` 是必要的：有東西等著畫卻剛好被擋下來時，
        // 沒有它就會睡到下一次取樣（最多 250 毫秒）才更新出去。
        let now = Instant::now();
        let far = Duration::from_secs(3600);
        let until_sample = app.next_sample_in(now);
        let until_clock = clock_tick.saturating_sub(now.duration_since(last_draw));
        // 操作中不把動畫算進醒來的理由，否則會為了一個馬上要被跳過的
        // 動畫格而空轉
        let until_anim = if interacting {
            settle.saturating_sub(now.duration_since(last_input))
        } else {
            app.next_animation_in(now)
        };
        let until_frame = if input_dirty {
            Duration::ZERO
        } else if bg_dirty {
            min_frame.saturating_sub(now.duration_since(last_draw))
        } else {
            far
        };
        let wait = until_sample
            .min(until_clock)
            .min(until_anim)
            .min(app.next_game_tick_in(now))
            .min(until_frame)
            .clamp(Duration::from_millis(1), Duration::from_millis(250));
        // 只是等，實際讀取在迴圈頂端 —— 輸入的入口只有一個。
        let _ = event::poll(wait)?;
    }
}

/// 互動式 sudo 授權。
///
/// **關鍵**：必須把終端機完整還原（離開 alternate screen、關掉 raw mode），
/// 讓 `sudo` 自己處理密碼提示。sysview 不讀、不存、不轉送任何密碼。
/// 完成後再重新進入 TUI。
fn authenticate(terminal: DefaultTerminal, app: &mut App) -> std::io::Result<DefaultTerminal> {
    // 先把終端機完整交還：離開 alternate screen、關掉 raw mode。
    // 之後那段時間 sysview 完全不碰終端機，sudo 想怎麼提示都行
    // （密碼、PAM、指紋、硬體金鑰都由它自己處理）。
    drop(terminal);
    ratatui::restore();
    // 明確清屏，避免 TUI 的殘影和 sudo 的提示疊在一起
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::MoveTo(0, 0),
    );

    println!();
    println!("sysview 需要管理員權限才能顯示這些資訊。");
    println!("接下來由 sudo 自己處理驗證 —— sysview 不會看到你的密碼。");
    if let Some(h) = app.privilege.helper_path() {
        println!("將執行：sudo -- {} capability", h.display());
    }
    println!();
    let _ = std::io::stdout().flush();

    let outcome = app.privilege.authenticate_interactive();

    // 這裡刻意**不呼叫** `terminal.clear()`。
    //
    // 兩個原因：
    //   1. `try_init()` 產生的是全新的 Terminal，兩個內部緩衝區都是空的，
    //      所以下一次 draw() 本來就會整頁重畫，不需要額外 clear。
    //   2. `Terminal::clear()` 會去查游標位置（送 ESC[6n 等回應）。
    //      剛從 sudo 回來時終端機的輸入狀態不一定乾淨，這個查詢會逾時，
    //      整個程式就會因為「使用者密碼打錯」這種小事而結束。
    let terminal = ratatui::try_init()?;
    match outcome {
        Ok(()) => {
            app.note("管理員功能已解鎖");
            app.request_admin(false);
        }
        Err(e) => app.note(format!("未能解鎖：{e}")),
    }
    Ok(terminal)
}

/// 跑的是 `~/.local/bin` 的私人版，而系統版也裝了 —— PATH 通常把
/// `~/.local/bin` 排在前面，先裝 `--user` 再請管理員裝全機版的人，會一直
/// 跑到家目錄那份舊的（沒有 Admin 頁、沒有後來的修正）而不自知。
fn shadowed_by_private_copy() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let private_dir = home.join(".local").join("bin");
    let system = std::path::Path::new("/usr/local/bin/sysview");
    if exe.starts_with(&private_dir) && system.is_file() {
        return Some(
            "你跑的是 ~/.local/bin/sysview（私人版），系統已裝有 /usr/local/bin/sysview；\
             要用系統版（含 Admin 頁）：rm ~/.local/bin/sysview"
                .to_owned(),
        );
    }
    None
}

fn translate(k: KeyEvent) -> Option<Key> {
    Some(match k.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        _ => return None,
    })
}

fn list_metrics() {
    println!("sysview 可解釋的 metric（用 sysview --explain <id> 看完整說明）：\n");
    for m in sysview::metrics::knowledge::METRICS {
        println!("  {:<24} {}", m.id, m.title);
    }
    println!("\n共 {} 個。", sysview::metrics::knowledge::METRICS.len());
}

fn explain_metric(id: &str) -> std::process::ExitCode {
    let Some(def) = sysview::metrics::lookup(id) else {
        eprintln!("找不到 metric {id:?}。用 --list-metrics 看全部。");
        // 給個相近的建議
        let close: Vec<&str> = sysview::metrics::knowledge::all_ids()
            .filter(|m| m.starts_with(id.split('.').next().unwrap_or("")))
            .collect();
        if !close.is_empty() {
            eprintln!("你是不是要找：{}", close.join(", "));
        }
        return std::process::ExitCode::from(2);
    };
    println!("{}  ({})", def.title, def.id);
    println!("{}", "─".repeat(60));
    println!("\nWhat is this?\n  {}", def.meaning);
    println!("\nFormula\n  {}", def.formula);
    println!("\nSource");
    for s in def.sources {
        println!("  {s}");
    }
    println!("\nPitfalls");
    for p in def.pitfalls {
        println!("  ! {p}");
    }
    println!("\nNative commands");
    for c in def.commands {
        println!("  $ {c}");
    }
    if !def.related.is_empty() {
        println!("\nRelated metrics\n  {}", def.related.join("  ·  "));
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_halves_the_frame_rate_while_the_sink_is_slow() {
        let frame = Duration::from_millis(16);
        let t0 = Instant::now();
        let mut p = Pacer::new();
        assert_eq!(p.min_frame(t0, frame), frame);
        // 正常的一格：畫 3 ms
        p.record(t0, Duration::from_millis(3), frame);
        assert_eq!(p.min_frame(t0, frame), frame);
        // flush 卡了 40 ms：接下來一秒降到一半
        p.record(t0, Duration::from_millis(40), frame);
        assert_eq!(
            p.min_frame(t0 + Duration::from_millis(500), frame),
            frame * 2
        );
        assert_eq!(
            p.min_frame(t0 + Duration::from_millis(1100), frame),
            frame,
            "終端機追上之後要恢復"
        );
    }

    const MIN_FRAME: Duration = Duration::from_millis(40);
    const SETTLE: Duration = Duration::from_millis(250);

    #[test]
    fn input_is_never_held_back_by_the_frame_limiter() {
        // 節流器是為了不讓閒置的動畫燒 CPU，不是用來讓按鍵排隊的。
        // 混在一起會讓每次按鍵最多被擋 40 毫秒。
        for since in [0u64, 1, 20, 39, 40, 500] {
            assert!(
                should_draw(true, false, Duration::from_millis(since), MIN_FRAME),
                "距上一幀 {since}ms 時，輸入造成的重畫被擋住了"
            );
        }
    }

    #[test]
    fn background_updates_are_throttled() {
        assert!(!should_draw(
            false,
            true,
            Duration::from_millis(0),
            MIN_FRAME
        ));
        assert!(!should_draw(
            false,
            true,
            Duration::from_millis(39),
            MIN_FRAME
        ));
        assert!(should_draw(
            false,
            true,
            Duration::from_millis(40),
            MIN_FRAME
        ));
    }

    #[test]
    fn nothing_dirty_draws_nothing() {
        assert!(!should_draw(
            false,
            false,
            Duration::from_secs(10),
            MIN_FRAME
        ));
    }

    #[test]
    fn the_animation_yields_while_the_user_is_working() {
        // 按住方向鍵捲動時，吉祥物整段時間都停著
        assert!(!animation_may_advance(Duration::from_millis(0), SETTLE));
        assert!(!animation_may_advance(Duration::from_millis(249), SETTLE));
        assert!(animation_may_advance(Duration::from_millis(250), SETTLE));
        assert!(animation_may_advance(Duration::from_secs(5), SETTLE));
    }
}
