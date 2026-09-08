//! 命令列輸出模式的行為。
//!
//! 這些是「別人第一次拿到這支程式會做的事」：導進 `jq`、接 `head`、
//! 存成檔案、丟進 cron。它們在別台機器上出錯的成本特別高 ——
//! 使用者會以為是程式壞了，而不是自己的管線。

use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_sysview")
}

/// 讀端提早關閉時，不可以 panic。
///
/// Rust 啟動時把 SIGPIPE 設成 `SIG_IGN`，於是寫進沒人讀的管線會變成
/// `EPIPE`，而 `println!` 遇到寫入錯誤就 panic：
///
/// ```text
/// $ sysview --json | jq .cpu
/// thread 'main' panicked: failed printing to stdout: Broken pipe (os error 32)
/// ```
///
/// `head` / `jq` / `less` 提早離開是**正常用法**（README 自己就寫了
/// `sysview --json | jq .`），正確的反應是安靜結束。修法是把 SIGPIPE
/// 設回 `SIG_DFL`（見 `collectors::util::restore_default_sigpipe`）。
///
/// 這個 bug 會不會發作取決於「輸出寫完」和「讀端關閉」誰先發生，所以
/// 測試不能只跑一次 —— 它以前是間歇性的，在 CI 上偶爾紅。
#[test]
fn output_modes_do_not_panic_when_the_reader_goes_away() {
    for mode in ["--snapshot", "--json", "--list-metrics"] {
        for attempt in 1..=3 {
            // stdout 接到一個立刻被丟掉的管線 = 讀端馬上關閉
            let mut child = Command::new(bin())
                .arg(mode)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn");
            // 把讀端丟掉：接下來每一次寫入都會踩到 EPIPE / SIGPIPE
            drop(child.stdout.take());
            let out = child.wait_with_output().expect("wait");
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(
                !err.contains("panicked") && !err.contains("Broken pipe"),
                "{mode} 第 {attempt} 次：讀端關掉之後 panic 了\n{err}"
            );
        }
    }
}

/// 沒有 TTY 時不帶參數要自己走快照，而不是卡住等終端機。
///
/// cron、`ssh host sysview`、CI 都是這條路徑。卡住的話是靜默的災難：
/// 沒有輸出、也沒有錯誤，只是永遠不結束。
#[test]
fn without_a_tty_it_prints_a_snapshot_instead_of_waiting() {
    let out = Command::new(bin())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("執行");
    assert!(out.status.success(), "沒有 TTY 時應該正常結束");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("CPU") && text.contains("MEM"),
        "沒有 TTY 時應該輸出快照，實際得到：{text:.200}"
    );
}

/// `--json` 一定要是合法 JSON —— 它存在的意義就是給別的程式吃。
#[test]
fn json_output_actually_parses() {
    let out = Command::new(bin())
        .arg("--json")
        .stdin(Stdio::null())
        .output()
        .expect("執行");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("--json 不是合法 JSON");
    for key in ["cpu", "memory", "processes"] {
        assert!(v.get(key).is_some(), "--json 少了 {key}");
    }
}

#[test]
fn the_login_banner_is_short_narrow_and_needs_nothing() {
    // 登入畫面是安裝時用 --banner 產成的靜態檔：沒有 TTY、沒有設定檔、沒有
    // 顏色也要能產；每一列不超過 80 欄（ssh 客戶端的最小公分母）。
    let out = Command::new(bin())
        .args(["--banner", "--no-color"])
        .env_remove("NO_COLOR")
        .env("XDG_CONFIG_HOME", "/nonexistent")
        .output()
        .expect("執行 --banner");
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf-8");
    assert!(!text.contains('\x1b'), "--no-color 不該有跳脫序列");
    assert!(text.contains("sysview"), "要有程式名字：\n{text}");
    assert!(text.contains("all in one"), "要有那句邀請：\n{text}");
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines.len() >= 10 && lines.len() <= 24,
        "登入畫面 {} 列，不像一個 banner",
        lines.len()
    );
    for l in &lines {
        assert!(
            unicode_width::UnicodeWidthStr::width(*l) <= 80,
            "超過 80 欄：{l:?}"
        );
    }
    // 有顏色的版本：只多了 SGR，內容一樣
    let colour = Command::new(bin())
        .arg("--banner")
        .env_remove("NO_COLOR")
        .env("XDG_CONFIG_HOME", "/nonexistent")
        .output()
        .expect("執行 --banner");
    let ctext = String::from_utf8(colour.stdout).expect("utf-8");
    assert!(ctext.contains("\x1b["), "預設應該帶顏色");
}
