# sysview 安全設計（CIA 分析）

這份文件用 **Confidentiality / Integrity / Availability** 三個面向說明
sysview 的安全模型、防線，以及已知的限制。

威脅模型的核心假設：

> **假設非特權的 sysview TUI 已經被攻陷。**
> 它是一般使用者身分執行的程式，攻擊者可以任意控制傳給 helper 的參數。
> 在這個前提下，`sysview-priv` 仍然必須是安全的。

---

## Confidentiality（機密性）

### 原則

> 一般使用者不得因為使用 sysview 而看到本來 Linux 權限不允許看的資訊。

sysview 沒有任何「提權捷徑」。非特權模式下的每一次讀取都是以呼叫者的身分
進行的，核心該擋的它就會擋。

### 具體防線

| 風險 | 防線 |
|---|---|
| 其他使用者的檔案路徑 | 非特權模式只掃自己的家目錄；跨使用者資料一律走 sudo helper |
| 行程的環境變數 | **完全不讀** `/proc/<pid>/environ`。那裡面幾乎一定有 API token、資料庫密碼 |
| 行程的命令列參數 | 只顯示呼叫者本來就讀得到的（`/proc/<pid>/cmdline` 的權限由核心控制）|
| `/proc/<pid>/fd` | 只在 socket 對應時讀，且只透過 helper；只取 inode 號碼，不解析目標內容 |
| GPU 行程歸屬 | NVML 只回傳呼叫者有權限看到的行程。UI 會標示「僅自己的」還是完整清單 |
| 家目錄內容 | 掃描只呼叫 `lstat`，**絕不開啟任何檔案內容** |
| 管理員查詢結果 | 只存在記憶體，**不寫任何暫存檔、不寫快取**。helper 只把「誰對哪個 PID 做了什麼」這種中繼資料記到 journald；查詢結果本身不記 |

### 特別說明：為什麼不顯示 environ

`/proc/<pid>/environ` 是管理員最容易誤觸的機密來源。實務上那裡面常有：

```
DATABASE_URL=postgres://user:password@host/db
AWS_SECRET_ACCESS_KEY=...
GITHUB_TOKEN=ghp_...
```

一個監控工具沒有任何正當理由把這些顯示在畫面上。`process-detail` 的
回應裡 `environ` 永遠是 `null`，並附上說明。

### 特別說明：不做封包擷取

網路的管理員擴充只回報**中繼資料**：每個使用者開了幾條連線、其中幾個在監聽、
各狀態的數量。不列 port、不列 PID、不列對端位址。

sysview **不**擷取封包、**不**看 payload、**不**攔截憑證。
那是 Wireshark 的領域，不是系統觀測工具該做的事。

### 暫存檔

sysview 不為監控或管理員資料建立任何暫存檔：沒有 `/tmp/sysview.json`、沒有 debug dump、
沒有 world-readable 的快取。管理員資料的生命週期就是 TUI 的記憶體。唯一會寫的檔是
彩蛋排行榜（見下），更新時在同一個目錄先寫暫存檔再 rename；暫存檔以 `O_EXCL` 建立、
不跟 symlink，名字撞了換下一個，不覆蓋任何已存在的檔。

### 彩蛋排行榜：唯一會寫的檔

恐龍彩蛋的排行榜是全機共用的，但**沒有任何特權**：每個人只寫自己的
`/var/lib/sysview/dino/<uid>.json`（0644），目錄是 `1777`（sticky，跟 `/tmp`
一樣），由 `make install` 建、sysview 自己不會建。別人的檔只讀，而且當
不可信輸入處理（不跟 symlink、檔名 uid 必須等於 owner、大小上限、JSON 壞了
跳過、名字清掉控制字元）。裡面沒有任何監控資料 —— 只有名字、分數、時間。
分數在使用者自己的行程裡算，所以**它是遊戲的榜，不是稽核資料**：能偽造
的是分數，不能偽造的是「從哪個帳號來的」（檔的 owner）。細節見 docs/dino.md。

---

## Integrity（完整性）

### 原則

> 預設唯讀。任何會改變系統狀態的動作都必須明確、可見、可稽核。

九個 helper 操作裡只有兩個會改狀態：`process-signal` 與 `renice`。

### 具體防線

**1. 窄介面（narrow allowlist）**

helper 只接受列舉好的操作，沒有 catch-all：

* 沒有 `exec`、`run`、`shell`、`eval`
* 不接受任何路徑參數 —— 家目錄是 helper 自己查 password database 得到的，
  **所以路徑穿越在架構上不可能發生**，不是靠檢查擋掉的
* 不接受任何指令字串。全程沒有 `sh -c`、沒有字串串接、沒有 shell
* 參數只接受 `--key value` 形式，不支援 `--key=value`、不支援短選項、
  不支援位置參數。同一參數給兩次直接拒絕
* 訊號只接受 `TERM` / `INT` / `HUP` 三個名字，**不接受數字**

**2. 沒有 SIGKILL**

刻意不提供。SIGKILL 不給行程清理的機會，對資料庫或寫入中的檔案很危險。
真的要用的人請自己 `kill -9` —— 那時責任明確在他自己身上。

**3. renice 只能調低優先度**

只接受 0–19。提高優先度（負值）是另一種資源濫用途徑，這版不開放。

**4. PID 重用防護（confused deputy）**

PID 會被重用。UI 一秒前看到的 PID 4321 可能已經結束，現在的 4321 是別的行程。

所以改狀態的操作**必須同時帶 `starttime`**（該行程建立時的開機以來 jiffies），
helper 在真的動手之前會重新讀 `/proc/<pid>/stat` 比對：

確認與 `kill` / `setpriority` 之間仍有幾微秒的空隙。要在那個空隙裡把同一個 PID
回收給別的行程，得先繞完整個 `pid_max`（64 位元預設 4194304），實務上不會發生；
`pidfd_send_signal` 只能收斂訊號那一半，`setpriority` 沒有 pidfd 版本，所以維持
這個設計，不為了對稱性引進第二套機制。

```rust
if st.starttime != starttime {
    bail!("PID {pid} 已經被重用 —— 拒絕操作");
}
```

`(pid, starttime)` 幾乎可以唯一識別一個行程，這關掉了最典型的
TOCTOU / confused deputy 缺口。

**5. 不對 PID 1 動手**

殺掉 init 會讓整台機器停擺。helper 直接拒絕。

**6. 二次確認**

UI 上任何改狀態的操作都會跳出確認框，而且：

* 顯示**實際會被影響的** PID、使用者、完整指令 —— 不是「你確定嗎？」
* 預設游標停在「取消」上，必須主動移到「確認」才能執行
* 目標是 root 的行程會有額外警告

**7. helper 不信任 TUI**

客戶端驗過的每個參數，helper 都會**再驗一次**。這是 defense in depth：
就算 TUI 被攻陷，helper 仍然安全。

**8. 檔案系統走訪的安全性**

per-user storage 掃描：

* 一律用 `symlink_metadata`（`lstat`），**不跟隨 symlink** ——
  避免無窮迴圈，也避免一個指向 `/etc` 的連結被算進使用者用量
* 不跨檔案系統邊界
* 只 `lstat`，絕不 `open`。開一個沒有寫入端的 FIFO 會讓整個掃描永久卡住
* 純 Rust 走訪，**不 fork `du`**，所以沒有任何指令注入面

**9. helper 安裝方式檢查**

每次呼叫前，客戶端都會重新檢查 helper：

* 必須是實體檔案（不是 symlink —— symlink 可被替換）
* **不可以有 setuid / setgid 位元**
* 在系統路徑上時，必須是 root 擁有、不可被 group/other 寫入

安裝器也會做同樣的檢查，裝錯會直接失敗。

**10. helper 路徑不可被環境變數改變**

候選路徑是編譯進 binary 的固定清單。沒有 `SYSVIEW_HELPER` 這種環境變數 ——
否則攻擊者只要設一個指向自己程式的路徑，就能讓 sudo 幫他跑。

---

## Availability（可用性）

### 原則

> sysview 自己絕不能成為 server 的負載來源。

### 實測（i7-13700 / 24 執行緒 / ~530 個行程）

| 項目 | 成本 |
|---|---|
| 單輪完整取樣 | 5.14 ms |
| 1 Hz 下的 CPU | 0.51% 單核 = **全機 0.021%** |
| 常駐記憶體（含 NVML） | ~21 MB |
| 常駐記憶體（`--no-gpu`） | **4.0 MB** |
| 每頁渲染 | 0.14 – 0.34 ms |

### 具體防線

**1. 各 collector 獨立的取樣間隔**

掃全部 `/proc/<pid>` 是最貴的動作（佔單輪成本的 65%）。行程清單本來就
不需要跟圖表一樣快，所以獨立節流：

| 資料 | 間隔 |
|---|---|
| CPU / 記憶體 / 磁碟 / 網路圖表 | 跟隨主更新率 |
| GPU（NVML，微秒等級） | ≥ 0.5 s |
| 行程清單 | **≥ 0.75 s**（即使主更新率是 0.2 s）|
| 掛載點 statvfs | 5 s |
| IP 位址、連線速度 | 10 s |
| 使用者儲存掃描 | **按需，絕不進入取樣迴圈** |

**2. 事件驅動的繪製**

畫面只在「取樣到新資料」「按了鍵」「視窗改變大小」時重繪，
不是固定 fps 空轉。閒置時 sysview 幾乎不消耗 CPU。

**3. NVML 而不是 fork nvidia-smi**

v1 每次輪詢都 fork 一個 `nvidia-smi`（實測 12.3 ms CPU）。
v2 直接呼叫 NVML（1.33 ms），**便宜約 10 倍**，而且沒有 fork 風暴。

代價是 NVML 的 dlopen 讓 RSS 多約 15 MB（那是 NVIDIA 驅動自己的緩衝區）。
不需要 GPU 監控的部署可以用 `--no-gpu` 換回 4 MB。

**4. NVML 只在有 NVIDIA 卡時才載入**

先讀 `/sys/bus/pci/devices` 確認有 class `0x03` 且 vendor `0x10de` 的裝置，
才做 dlopen。沒有 NVIDIA 的機器完全不付這個代價。

**5. 昂貴診斷只在壞掉時才跑**

`dpkg-query` / `dkms status` / `mokutil` 只有在偵測到有 NVIDIA 卡
但 NVML 起不來時才執行一次，正常情況下完全不 fork 任何行程。

**6. 掃描有預算上限且可取消**

per-user storage 掃描：

* 有 deadline（整體 90 秒，平均分給每個使用者）
* 有 entry 上限（300 萬）
* 有遞迴深度上限（64 層）
* 可取消
* **任何一項觸發就標記為 `partial`**，UI 明確顯示，不會讓使用者誤信不完整的數字

**7. 記憶體有硬上限**

歷史序列是固定長度的 ring buffer（預設 240 點，設定檔上限 4096）。
長時間執行不會讓記憶體無限成長。

**8. 執行緒有上限**

管理員查詢最多同時四個（四個分頁各一個），不是無上限地開執行緒。

**9. 每一次 sudo 呼叫都有逾時**

`sudo` 平常回應是毫秒等級，但在 LDAP / AD / 網路認證的環境下**可能阻塞**。
一個監控工具不能因為認證後端慢就整個卡死，所以：

| 操作 | 逾時 |
|---|---|
| `capability` 探測 | 5 s |
| 讀取類操作 | 30 s |
| `storage-user-detail` | 90 s |
| `storage-users`（掃全部家目錄） | 150 s |

`std::process::Command::output()` 沒有逾時機制，所以這裡自己實作：
兩條執行緒把 stdout/stderr 抽乾（避免管線塞滿造成死結），主執行緒輪詢
`try_wait` 直到期限，逾時就 `kill` 並 `wait`（不留殭屍行程）。
有測試實際起一個 `sleep 60` 驗證它真的被殺掉，也有測試用 200 KB 的輸出
驗證不會死結。

唯一沒有逾時的是**互動式驗證**——使用者正在輸入密碼，不能在他打到一半時
把 sudo 殺掉。那時終端機已經完整交還給 sudo，Ctrl-C 由 sudo 自己處理。

**10. 啟動時的授權探測是非同步的**

`capability` 探測跑在背景執行緒，狀態放在 `Arc<Mutex<PrivilegeState>>`。
就算 sudo 要 5 秒才回應，TUI 也是立刻出現，使用者最多看到一瞬間的
`admin: locked` 然後自動更新。Mutex 中毒時一律回報「不可用」——
fail closed。

**11. 不做 DNS 查詢**

網路頁只顯示 IP，不做反解。DNS 逾時會讓 UI 卡住。

**12. 不會因為系統壞掉而崩潰**

`/proc`、`/sys`、GPU、裝置、行程、掛載點全都是隨時會消失的東西。
行程可能在 `readdir` 與 `open` 之間結束 —— 這是**正常情況**。

整個專案不用 `unwrap()` / `expect()` 處理執行期系統資訊，一律轉成
`n/a` / `permission denied` / `unsupported` 顯示。

---

## 設定檔不能影響授權

`~/.config/sysview/config.toml` 只管外觀與取樣頻率。

裡面**沒有** `admin = true`、`allow_root`、`helper_path` 這類欄位，
而且用 `deny_unknown_fields` 解析 —— 有人手動加了會直接報錯，
不會讓使用者誤以為自己開了什麼開關。

一個使用者能編輯的檔案，永遠不該是安全決策的依據。

---

## 已知限制

誠實列出還沒做到的：

* **沒有 polkit 整合。** 這版只走 sudo。架構上可以再加一個 backend，但還沒做。
* **helper 的 IPC 是 argv + stdout JSON，不是 socket。** 夠用且攻擊面小，
  但無法做串流進度回報（掃描進度目前只能靠經過時間估算）。
* **cgroup 感知只做到偵測與顯示配額**，還沒把 CPU 使用率換算成「相對於配額」。
  容器裡看到的 CPU% 仍是相對於宿主機全部核心的。這個限制在 UI 上有標示。
* **AMD backend 沒有實機驗證。** 程式碼依 amdgpu sysfs ABI 撰寫，但開發機
  上只有 NVIDIA 與 Intel。README 有據實標示。
* **多 GPU 沒有實機驗證。** 程式碼有處理，但沒有實體多卡機器可測。
* **`--json` 沒有管理員模式。** 這是刻意的：非互動輸出不做 sudo 提權，
  避免在 cron / pipe 情境下意外提權。

---

## 回報安全問題

如果你發現：

* helper 接受了不該接受的參數
* 一般使用者透過 sysview 看到了本來看不到的資訊
* helper 被裝成 setuid
* 有辦法讓 helper 執行任意指令

請直接回報，不要公開揭露。
