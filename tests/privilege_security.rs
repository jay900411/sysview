//! 特權層的端到端安全測試。
//!
//! 這裡實際執行編譯出來的 `sysview-priv` binary，驗證它在**真實情況下**
//! 拒絕它該拒絕的東西 —— 而不只是單元測試裡的函式回傳值。

use std::path::PathBuf;
use std::process::Command;

fn helper() -> PathBuf {
    // 測試執行檔在 target/<profile>/deps/，helper 在 target/<profile>/
    let mut p = std::env::current_exe().expect("current_exe");
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.join("sysview-priv")
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(helper())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("無法執行 {}: {e}", helper().display()));
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

#[test]
fn helper_binary_exists() {
    assert!(helper().is_file(), "helper 未編譯：{}", helper().display());
}

#[test]
fn helper_is_not_setuid_in_build_output() {
    use std::os::unix::fs::PermissionsExt;
    let md = std::fs::metadata(helper()).expect("metadata");
    let mode = md.permissions().mode();
    assert_eq!(mode & 0o4000, 0, "helper 絕不可帶 setuid 位元");
    assert_eq!(mode & 0o2000, 0, "helper 絕不可帶 setgid 位元");
}

#[test]
fn refuses_to_run_without_root() {
    if is_root() {
        eprintln!("以 root 執行測試，略過此項");
        return;
    }
    let (code, stdout, _) = run(&["capability"]);
    assert_ne!(code, 0, "非 root 執行必須失敗");
    assert!(stdout.contains("error"), "必須回傳結構化錯誤：{stdout}");
    assert!(stdout.contains("root"), "錯誤訊息應說明需要 root：{stdout}");
    // 絕不可洩漏任何系統資料
    assert!(!stdout.contains("/home/"), "拒絕時不可洩漏任何路徑");
    assert!(!stdout.contains("users"), "拒絕時不可回傳任何資料");
}

#[test]
fn every_operation_refuses_without_root() {
    if is_root() {
        return;
    }
    let ops: &[&[&str]] = &[
        &["capability"],
        &["storage-users"],
        &["storage-user-detail", "--user", "root"],
        &["user-memory"],
        &["gpu-users"],
        &["socket-map"],
        &["process-detail", "--pid", "1"],
        &[
            "process-signal",
            "--pid",
            "1",
            "--starttime",
            "1",
            "--signal",
            "TERM",
        ],
        &["renice", "--pid", "1", "--starttime", "1", "--nice", "5"],
    ];
    for op in ops {
        let (code, out, _) = run(op);
        assert_ne!(code, 0, "{op:?} 在非 root 下必須失敗");
        assert!(out.contains("error"), "{op:?} 必須回傳錯誤");
    }
}

#[test]
fn rejects_arbitrary_command_execution_attempts() {
    // 就算有 root，這些也必須被拒絕
    let attacks: &[&[&str]] = &[
        &["exec", "--cmd", "id"],
        &["run", "--command", "/bin/sh"],
        &["shell"],
        &["sh", "-c", "id"],
        &["eval", "id"],
        &["capability; id"],
        &["storage-users", "--path", "/etc/shadow"],
        &["storage-user-detail", "--user", "../../root"],
        &["storage-user-detail", "--user", "$(id)"],
        &["storage-user-detail", "--user", "a;id"],
        &["process-detail", "--pid", "0"],
        &["process-detail", "--pid", "-1"],
        &[
            "process-signal",
            "--pid",
            "1",
            "--starttime",
            "1",
            "--signal",
            "KILL",
        ],
        &[
            "process-signal",
            "--pid",
            "1",
            "--starttime",
            "1",
            "--signal",
            "9",
        ],
        &["renice", "--pid", "1", "--starttime", "1", "--nice", "-20"],
    ];
    for a in attacks {
        let (code, out, _) = run(a);
        assert_ne!(code, 0, "{a:?} 必須被拒絕，實際輸出：{out}");
    }
}

#[test]
fn output_is_always_valid_json() {
    // 不論成功或失敗，輸出都必須是可解析的 JSON —— 客戶端才不會誤判
    for args in [
        vec!["capability"],
        vec!["nonexistent-op"],
        vec!["process-detail", "--pid", "0"],
        vec![],
    ] {
        let (_, stdout, _) = run(&args);
        assert!(!stdout.trim().is_empty(), "{args:?} 沒有任何輸出");
        let v: serde_json::Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("{args:?} 的輸出不是合法 JSON: {e}\n{stdout}"));
        assert!(v["protocol_version"].is_number(), "缺少協定版本");
        assert!(v["status"].is_string(), "缺少 status 欄位");
    }
}

#[test]
fn client_fails_closed_without_sudo_credentials() {
    use sysview::privilege::{Operation, PrivilegeClient};
    let client = PrivilegeClient::new();
    // 測試環境不該有有效的 sudo 快取；就算有，也必須是明確的成功或失敗，
    // 絕不能是「部分成功」或回傳空資料當成成功。
    match client.run(&Operation::StorageUsers) {
        Ok(v) => {
            // 若真的有 sudo 快取，資料必須是完整結構
            assert!(v["users"].is_array(), "成功時必須回傳結構化資料");
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "失敗必須有明確原因");
        }
    }
}

#[test]
fn client_never_puts_secrets_on_the_command_line() {
    use sysview::privilege::protocol::{Operation, SafeSignal};
    let ops = [
        Operation::Capability,
        Operation::StorageUserDetail {
            user: "alice".into(),
        },
        Operation::ProcessSignal {
            pid: 1,
            starttime: 1,
            signal: SafeSignal::Term,
        },
    ];
    for op in ops {
        for arg in op.to_argv() {
            let lower = arg.to_ascii_lowercase();
            for forbidden in ["password", "passwd", "token", "secret", "key"] {
                assert!(
                    !lower.contains(forbidden),
                    "argv 不該出現 {forbidden:?}：{arg}"
                );
            }
        }
    }
}

#[test]
fn protocol_has_no_path_or_command_fields() {
    // 架構層級的保證：只要協定沒有這些欄位，對應的攻擊面就不存在
    use sysview::privilege::protocol::{Operation, SafeSignal};
    let all = [
        Operation::Capability,
        Operation::StorageUsers,
        Operation::StorageUserDetail { user: "u".into() },
        Operation::UserMemory,
        Operation::GpuUsers,
        Operation::SocketMap,
        Operation::ProcessDetail { pid: 1 },
        Operation::ProcessSignal {
            pid: 1,
            starttime: 1,
            signal: SafeSignal::Term,
        },
        Operation::Renice {
            pid: 1,
            starttime: 1,
            nice: 1,
        },
    ];
    for op in all {
        let json = serde_json::to_string(&op).unwrap();
        assert!(!json.contains("path"), "協定不可有 path 欄位：{json}");
        assert!(!json.contains("cmd"), "協定不可有 cmd 欄位：{json}");
        assert!(!json.contains("command"), "協定不可有 command 欄位：{json}");
        assert!(!json.contains('/'), "協定不可出現路徑：{json}");
    }
}

#[test]
fn only_two_operations_can_mutate_state() {
    use sysview::privilege::protocol::{Operation, SafeSignal};
    let all = [
        (Operation::Capability, false),
        (Operation::StorageUsers, false),
        (Operation::StorageUserDetail { user: "u".into() }, false),
        (Operation::UserMemory, false),
        (Operation::GpuUsers, false),
        (Operation::SocketMap, false),
        (Operation::ProcessDetail { pid: 1 }, false),
        (
            Operation::ProcessSignal {
                pid: 1,
                starttime: 1,
                signal: SafeSignal::Term,
            },
            true,
        ),
        (
            Operation::Renice {
                pid: 1,
                starttime: 1,
                nice: 1,
            },
            true,
        ),
    ];
    let mutations = all.iter().filter(|(_, m)| *m).count();
    assert_eq!(mutations, 2, "會改狀態的操作只該有兩個");
    for (op, expected) in &all {
        assert_eq!(
            op.is_mutation(),
            *expected,
            "{} 的 mutation 判定錯誤",
            op.name()
        );
    }
}

#[test]
fn running_the_tui_as_root_is_discouraged_not_silently_accepted() {
    assert!(
        !sysview::privilege::ROOT_WARNING.is_empty(),
        "必須有明確的 root 警告文字"
    );
    assert!(sysview::privilege::ROOT_WARNING.contains("sudo"));
}
