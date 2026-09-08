# 手動測試

這些測試需要真實的 pty 或 sudo 互動，沒辦法放進 `cargo test`。

先裝 pyte（終端機模擬器，只用來驗證，不是執行期相依）：

```bash
pip3 install --user pyte
```

## PTY 截圖驗證

在假終端機裡跑起 sysview，逐頁截圖，檢查有沒有 panic、框線有沒有被
中文撐破、有沒有寫出邊界。

```bash
cargo build --release
python3 tests/manual/pty_screenshot.py          # 產生 v2_shots.txt
python3 tests/manual/view_screenshot.py "3 GPU" 0 190   # 按顯示欄位切片檢視
```

`view_screenshot.py <頁面名稱> <起始欄> <結束欄>` 會正確處理全形字寬度
（直接用 `cut -c` 會把 UTF-8 切壞）。

## sudo 流程驗證

**這個要手動跑**，因為需要真的輸入密碼。

```bash
sysview
# 按 A 進管理員頁 → 顯示 "locked"
# 按 u → 應該完整離開 alternate screen，把畫面交給 sudo
# 輸入密碼 → 應該回到 TUI，管理員資料開始載入
```

### 已知過的回歸情境

按 `u` 之後**故意把密碼打錯三次**，sudo 放棄之後 TUI 必須完整恢復。

這裡曾經有 bug：重新進入 TUI 時呼叫 `Terminal::clear()` 會去查游標位置
（送 `ESC[6n` 等回應），剛從 sudo 回來時輸入狀態不乾淨會逾時，
整個程式就因為「密碼打錯」這種小事結束了。

現在的做法是不呼叫 `clear()` —— `try_init()` 給的是全新 Terminal，
下一次 `draw()` 本來就會整頁重畫。

驗證腳本（不需要正確密碼）：

```bash
python3 - <<'PY'
# 見 git 歷史裡的 sudo_recover.py；重點是送三次錯誤密碼後
# 確認程式仍活著、切頁仍有輸出、畫面上還有 sysview 頁首
PY
```

## 沒有 GPU / 沒有 NVIDIA 驅動的機器

```bash
# 模擬 NVML 載入失敗
mkdir -p /tmp/fakelib && echo x > /tmp/fakelib/libnvidia-ml.so.1
LD_LIBRARY_PATH=/tmp/fakelib sysview
# GPU 頁應顯示診斷，其他頁完全正常
```
