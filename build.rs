//! 把 git 的 commit 放進 `--version`，讓「你手上跑的到底是哪一版」有答案。
//!
//! 沒有 git（tarball、別人的機器）就寫 `unknown`，絕不讓建置失敗。
//! 只在 HEAD 或分支參照變動時重跑，不會每次都執行 git。

use std::process::Command;

fn main() {
    let rev = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=SYSVIEW_GIT_REV={rev}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");
    println!("cargo:rerun-if-changed=build.rs");
}
