#!/usr/bin/env bash
#
# sysview 安裝腳本。
#
# 做兩件事：
#   1. 編譯 release 版（不需要 root）
#   2. 安裝：
#        預設     裝到 /usr/local，全機共用（需要 root，會問一次 sudo 密碼）
#        --user   裝到 ~/.local，只給自己用（完全不需要 root）
#
# **這個腳本不會修改你的 sudo 政策。** 它不會寫 /etc/sudoers.d、
# 不會建立 NOPASSWD 規則。管理員擴充功能會直接沿用你系統現有的 sudo 設定。
# 想收緊或放寬，請自己看 docs/sudo.md。

set -euo pipefail
cd "$(dirname "$0")"

PREFIX="${PREFIX:-/usr/local}"
MODE=system

usage() {
    cat <<'USAGE'
用法：./install.sh [--user]

  （不加參數）  編譯後用 sudo 裝到 /usr/local，全機所有帳號都能用，
                並且安裝 Admin 頁需要的特權 helper。

  --user        編譯後只裝給自己，裝到 ~/.local —— **完全不需要 root**。
                七個一般頁面（總覽 / CPU / 記憶體 / GPU / 儲存 / 網路 /
                行程）全部可用；Admin 頁不會啟用，因為那需要一個 root
                擁有的 helper（家目錄裡的 helper 永遠不會被採用，否則
                等於把 root 送出去）。

  PREFIX=/path  改安裝位置（system 模式）。
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --user)    MODE=user; shift ;;
        -h|--help) usage; exit 0 ;;
        *)         usage; exit 1 ;;
    esac
done
RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; DIM=$'\033[2m'; OFF=$'\033[0m'

say()  { printf '%s==>%s %s\n' "$GREEN" "$OFF" "$*"; }
warn() { printf '%s!!%s  %s\n' "$YELLOW" "$OFF" "$*"; }
die()  { printf '%sxx%s  %s\n' "$RED" "$OFF" "$*" >&2; exit 1; }

# ── 找出 cargo ──────────────────────────────────────────────────────────
# rustup 如果是用 --no-modify-path 裝的，或者你還沒重開 shell，
# ~/.cargo/bin 就不會在 PATH 裡。這裡自己去標準位置找，不要因為這種
# 小事就叫使用者重裝。
find_cargo() {
    if command -v cargo >/dev/null 2>&1; then
        return 0
    fi
    # rustup 產生的 env 檔會把 ~/.cargo/bin 加進 PATH
    for env_file in "${CARGO_HOME:-$HOME/.cargo}/env" "$HOME/.cargo/env"; do
        if [ -f "$env_file" ]; then
            # shellcheck disable=SC1090
            . "$env_file"
            if command -v cargo >/dev/null 2>&1; then
                say "從 $env_file 載入工具鏈"
                return 0
            fi
        fi
    done
    # 最後直接看 bin 目錄
    for dir in "${CARGO_HOME:-$HOME/.cargo}/bin" "$HOME/.cargo/bin" /usr/local/cargo/bin; do
        if [ -x "$dir/cargo" ]; then
            export PATH="$dir:$PATH"
            say "在 $dir 找到工具鏈（它不在你的 PATH 裡）"
            return 0
        fi
    done
    return 1
}

if ! find_cargo; then
    die "找不到 cargo。請先安裝 Rust：
     curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   （安裝在使用者家目錄，不需要 root）"
fi

rustc_ver=$(rustc --version | awk '{print $2}')
say "工具鏈 rustc $rustc_ver  ($(command -v cargo))"

# 如果是我們自己找到的，提醒使用者把它加進 shell 設定檔，
# 否則下次他自己下 cargo 指令還是會找不到
if ! grep -qs 'cargo/env\|cargo/bin' "$HOME/.bashrc" "$HOME/.profile" "$HOME/.zshrc" 2>/dev/null; then
    NEEDS_PATH_HINT=1
fi

if [ "$(uname -s)" != "Linux" ]; then
    die "sysview 只支援 Linux（資料來源是 /proc 與 /sys）"
fi

# ── 編譯 ────────────────────────────────────────────────────────────────
# 走 `make build` 而不是直接 `cargo build`：make 會在建完之後記下兩個執行檔
# 的 sha256，而 `make install` 會拿那個章去確認「要裝的就是剛剛乾淨建出來
# 的那份」。直接 cargo build 不會寫章 —— 之前跑過 `cargo test --all-features`
# 的話，target/release/ 裡放的是**帶 test-support 的版本**，安裝就會被擋下來
# 而且看起來像是這個腳本壞了。
say "編譯 release 版（第一次會比較久）"
make build

for b in sysview sysview-priv; do
    [ -x "target/release/$b" ] || die "編譯後找不到 target/release/$b"
done
size_main=$(stat -c%s target/release/sysview)
size_priv=$(stat -c%s target/release/sysview-priv)
printf '%s    sysview      %s KB\n' "$DIM" "$((size_main / 1024))"
printf '    sysview-priv %s KB%s\n' "$((size_priv / 1024))" "$OFF"

# ── 安裝 ────────────────────────────────────────────────────────────────
# 編譯已經以你自己的身分完成了。接下來只是複製檔案，
# 所以 sudo make install 不會（也不該）重新編譯。
if [ "$MODE" = user ]; then
    say "安裝到 ~/.local（不需要 root）"
    echo
    printf '%s  ~/.local/bin/sysview                只有你自己會用到%s\n\n' "$DIM" "$OFF"
    make install-user
    PREFIX="$HOME/.local"
else
    say "安裝到 $PREFIX（只複製檔案，不重新編譯；需要 sudo 密碼）"
    echo
    printf '%s  %s/bin/sysview                    一般使用者執行，不需要 root\n' "$DIM" "$PREFIX"
    printf '  %s/libexec/sysview/sysview-priv   root:root 0755，不帶 setuid%s\n' "$PREFIX" "$OFF"
    printf '  /var/lib/sysview/dino               1777（sticky）：彩蛋排行榜，每人只寫自己的 <uid>.json%s\n\n' "$OFF"

    if ! sudo -v 2>/dev/null; then
        die "這台機器上你沒有 sudo 權限（或 sudo 拒絕了）。
   兩個選擇：
     1. 只裝給自己，不需要 root：  ./install.sh --user
     2. 請管理員代裝：             make && sudo make install"
    fi
    sudo make install PREFIX="$PREFIX"
    # 先裝過 --user 的人：~/.local/bin 在 PATH 前面，會遮住剛裝的系統版
    if [ -x "$HOME/.local/bin/sysview" ]; then
        warn "你的 ~/.local/bin/sysview（私人版）會遮住剛裝的系統版 —— PATH 先找到它。"
        warn "  刪掉就會用系統版：  rm ~/.local/bin/sysview"
    fi
fi

# ── 驗證 ────────────────────────────────────────────────────────────────
say "驗證"
hash -r
if ! command -v sysview >/dev/null 2>&1; then
    warn "$PREFIX/bin 不在你的 PATH 裡。加到 shell 設定檔："
    warn "  export PATH=\"$PREFIX/bin:\$PATH\""
else
    installed=$(command -v sysview)
    echo "    $installed  ($(sysview --version))"
fi

if [ "$MODE" = user ]; then
    echo "    Admin 頁未啟用（沒有 root 擁有的 helper）—— 其餘七頁完全不受影響"
else
    # helper 絕不可以是 setuid —— 真的裝成那樣的話是嚴重安全問題
    helper="$PREFIX/libexec/sysview/sysview-priv"
    mode=$(stat -c '%a' "$helper")
    case "$mode" in
        4*|2*|6*) die "helper 帶有 setuid/setgid 位元（mode $mode）。這是嚴重安全問題，請回報。" ;;
    esac
    echo "    helper mode $mode $(stat -c '%U:%G' "$helper")"

    # 直接執行 helper 必須被拒絕
    if "$helper" capability 2>/dev/null | grep -q '"status":"ok"'; then
        die "helper 在非 root 下竟然執行成功。這是嚴重安全問題，請回報。"
    fi
    echo "    helper 在非 root 下正確拒絕執行 ✓"
fi

cat <<EOF

$(printf '%s' "$GREEN")完成。$(printf '%s' "$OFF")

  sysview                 開啟儀表板
  sysview --snapshot      純文字快照（可導向檔案 / 給 cron 用）
  sysview --json | jq .   機器可讀輸出
  sysview -v gpu          直接開 GPU 頁
  sysview --no-gpu        不載入 NVML，常駐記憶體少約 15 MB

  在任何數字上按 $(printf '%s' "$GREEN")e$(printf '%s' "$OFF") 可以看到它的意義、來源、算式、陷阱與原生指令。
$(if [ "$MODE" = user ]; then
    printf '  Admin 頁（按 A）需要 root 擁有的 helper —— 請管理員跑一次 sudo make install。'
  else
    printf '  按 %sA%s 進入管理員擴充功能（會用 sudo 驗證，sysview 不碰你的密碼）。\n' "$GREEN" "$OFF"
    printf '  想讓 ssh 登入畫面出現 logo 與吉祥物：sudo make install-motd（uninstall-motd 移除）。'
  fi)

$(printf '%s' "$DIM")管理員權限完全沿用系統現有的 sudo 政策。
本腳本沒有修改 /etc/sudoers 或 /etc/sudoers.d。
若要限制或放寬 sysview 的 helper，請見 $PREFIX/share/doc/sysview/sudo.md$(printf '%s' "$OFF")
EOF

# 這段刻意放在 heredoc 之外：$( ) 會吃掉結尾換行，寫在裡面會和上一段黏在一起。
if [ "${NEEDS_PATH_HINT:-0}" = 1 ]; then
    echo
    warn "~/.cargo/bin 不在你的 PATH 裡。"
    warn "要以後能直接下 cargo 指令，把這行加到 ~/.bashrc："
    printf '        . "$HOME/.cargo/env"\n'
fi
