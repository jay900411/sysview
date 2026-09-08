#!/usr/bin/env bash
# 把 sysview 裝到全機器共用的位置，讓所有使用者都能直接打 `sysview`
set -euo pipefail
cd "$(dirname "$0")"
PREFIX="${PREFIX:-/usr/local}"

echo "==> 編譯"
make clean >/dev/null 2>&1 || true
make
echo "    產物: $(ls -l sysview | awk '{print $5}') bytes"

echo "==> 安裝到 $PREFIX/bin （需要 sudo 密碼）"
sudo make install PREFIX="$PREFIX"

echo "==> 驗證"
hash -r
command -v sysview
sysview --version
echo
echo "完成。任何使用者現在都可以直接執行：sysview"
echo "  sysview            互動儀表板"
echo "  sysview -i 0.5     每 0.5 秒更新"
echo "  sysview -v gpu     直接開 GPU 頁"
echo "  sysview --snapshot 純文字快照（可導向檔案 / 給 cron 用）"
