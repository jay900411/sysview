# sysview 設計說明

這份文件是 README 的長版：設計取捨、量測數字、實作細節。只想安裝與使用的話，README 就夠了。

## 這不是另一個 btop

btop、bottom、nvtop、Glances 都很好，sysview 沒打算取代它們。
它想解決的是另外三件事：

**1. 數字的來歷（metric provenance）**

在任何數字上按 `e`：

```
╭─┤ GPU Utilization ├────────────────────────────┤ gpu.util · % ├╮
│ What is this?                                                  │
│ 取樣區間內，GPU 上至少有一個 kernel 在執行的時間比例。         │
│                                                                │
│ Formula                                                        │
│ 驅動統計的「有 kernel 活動」時間 ÷ 取樣區間                    │
│                                                                │
│ Source                                                         │
│   NVML: nvmlDeviceGetUtilizationRates                          │
│                                                                │
│ Pitfalls                                                       │
│   ! 這不是 CUDA core 佔用率。跑滿 100% 不代表算力用好用滿 ——   │
│     一個只用了 1% SM 的 kernel 一直跑，也會顯示 100%。         │
│   ! Tensor Core 使用率完全不在這個數字裡。                     │
│   ! 訓練時如果 util 在 100% 與 0% 之間跳動，瓶頸通常在         │
│     資料載入而不是 GPU。                                       │
│                                                                │
│ Native commands                                                │
│   $ nvidia-smi dmon                                            │
│   $ nvtop                                                      │
│   $ ncu                                                        │
│                                                                │
│ Related metrics                                                │
│ gpu.vram · gpu.power · gpu.clock · gpu.process                 │
╰────────────────────────────────────────────────────────────────╯
```

39 個 metric 都有這一頁：意義、資料來源、算式、**容易誤解的地方**、
對應的原生指令、該一起看的其他 metric。

目標不是讓你依賴 sysview，是讓你**學得會 Linux**。

**2. 診斷，不只是顯示**

GPU 驅動掛掉時，多數工具只會說「nvidia-smi failed」。sysview 會查出原因：

```
╭─┤ NVIDIA Diagnostics ├──────────────────── NVML 目前不可用 ├╮
│ ✗ NVIDIA 核心模組沒有載入                                   │
│ · PCI 上偵測到 NVIDIA: 0000:01:00.0 (device 0x2782)         │
│ ✗ kernel module `nvidia` 未載入                             │
│ ✗ /dev/nvidia* 裝置節點: 不存在                             │
│ ✗ 同時裝了多個 nvidia-dkms 版本 (545, 550) — 會互相打架     │
│ ✗ 模組只替 6.8.0-40-generic 編過，但目前跑的是 6.8.0-90 —   │
│   核心升級後沒重建                                          │
│ · 目前核心: 6.8.0-90-generic                                │
╰─────────────────────────────────────────────────────────────╯
```

**3. 安全的管理員擴充**

`哪個 user 吃掉磁碟？誰佔著 GPU？誰在吃記憶體？` —— 這些問題需要 root，
但**不該為此把整個 TUI 用 root 跑**。sysview 用一個最小化的 helper 經 sudo 取得，
整支 TUI 永遠是你自己的身分。詳見 [docs/sudo.md](docs/sudo.md)。

---


## 視覺系統

sysview 有一套自己的視覺語言，不是隨手加的顏色。設計原則只有一條：
**它永遠不能贏過資料**。

### Pattern —— 這不是裝飾

組成類長條的每一段都有自己的**紋理**，不是只有顏色：

```
█████████▓▓▓▓⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿⣿••••••••••••••••••
█ used 5.76 GB  ▓ buffers 3.17 GB  ⣿ cache 34.6 GB  • free 19.7 GB
```

理由很實際：顏色會失效。單色終端、SSH 進到只有 8 色的機器、色盲讀者 ——
這幾種情況下純色長條會退化成一整條一樣的方塊，「哪一段是 cache」就讀不出來了。
紋理是一條跟顏色互相獨立的通道。

所以 **pattern 不受 `decorations = off` 影響**。裝飾可以關，encoding 不能關。
有測試在守這條線。

七種紋理：`solid` `shade` `dot` `braille` `line` `hatch` `digital`。
它們的輕重本身也是語意 —— 實心是「已經用掉拿不回來」，點是「空的」。

### 內建點陣字型

A–Z 與 0–9，共 36 個 5×7 glyph，編進二進位檔，不依賴 figlet。
外部工具意味著多一次 fork，而且不同機器裝的字型不一樣，
同一份設定畫出來會不同 —— 那違反 sysview「同樣的狀態給同樣的畫面」的原則。

5×7 是能同時把 `O`/`0`、`I`/`1`、`S`/`5`、`B`/`8`、`Z`/`2` 分清楚的最小尺寸，
而品牌代號常常混用字母和數字。有測試在守。

Logo 依空間降級：`點陣 → 半高點陣 → 純文字 → 不畫`。
就算你明確指定 `logo = "pixel"`，畫面放不下時它仍然會降級 —— 版面不能爆。

### 吉祥物

狐狸與鹿，各五個姿態，用**四分格**（一格 2×2 子像素）畫成 56 欄 × 16 列的剪影。
四分格比上下半格的橫向解析度加倍，腿的關節、鹿角的分叉、頭部的線條都留得住；
兩者同屬 U+2580 區段（CP437 時代就有），字型支援沒有差別。
不依賴任何圖片協定（sixel / kitty），SSH 到哪台機器都畫得出來。

| 姿態 | 什麼時候出現 |
|---|---|
| `observe` 安靜地看著 | 一切正常 |
| `explore` 走動 | 有負載但健康 |
| `proceed` 低頭專注 | 高負載 |
| `rest` 趴著休息 | 系統很閒超過 20 秒 |
| `return` 回望 | 剛從吃緊狀態恢復 |

剪影是**從 concept 圖直接萃取**的：切掉卡片外框、標題與說明文字，
裁到動物的外接框，再用面積平均降採樣到 56 欄。

中間試過改用多邊形重新描邊。線條確實比較滑順，但也把原圖的個性磨掉了 ——
腿的關節、身體上的分隔線、頭部的細節全變成一團光滑的剪影。
那些看起來像「破洞」的地方不是雜訊，是原作者畫的線。所以最後還是回到萃取。

**只有一個尺寸。** 降到 28 欄時細腿會碎成一格一格，
與其塞一隻醜的進去，不如不畫 —— 空間不夠就整隻不出現。

動畫是「一張點陣圖 + 幾個很小的差異」：眨眼關掉四個點，
呼吸讓剪影的**最上緣**多長半格。

呼吸本來是把整隻上移一像素，但那等於每格重畫半隻動物 ——
實測讓終端機輸出從 0.2 KB/s 跳到 1.9 KB/s。只動輪廓約只碰 50 格，
而且看起來比整隻上下跳更像呼吸。

**動畫時鐘與採樣時鐘是分開的。** 不管吉祥物跑多快，
`/proc`、NVML、行程掃描的頻率都不會被動到一分一毫。
fps 硬性夾在 1–8，因為這是裝飾，不值得為它燒 CPU。有測試在守這條。

吉祥物只出現在**不跟資料搶空間**的地方：

| 在哪 | 條件 |
|---|---|
| 說明頁（按 `?`） | 154 欄以上。**這裡最可靠** —— 一個鍵就到，而且沒有監控資料要讓位 |
| Overview 的 Highlights 卡片 | 150×40 以上（實測門檻，有測試守著） |
| Admin 鎖定畫面 | 夠寬時，右側 |
| 啟動畫面 | 預設就會出現（1.5 秒後自己消失，任何鍵跳過）|

`mascot = "auto"` 就是狐狸。本來讓它依系統狀態換種類，
但那很難預期 —— 使用者不知道自己什麼時候看得到哪一隻。
跟著狀態走的是**姿態**，種類固定下來比較好懂。

### 裝飾層級與 responsive

裝飾是一個有序的層級 `None < Minimal < Standard < Full`，
每一幀依**整個終端機**的大小算一次：

| 終端機 | 層級 | 看得到什麼 |
|---|---|---|
| ≥ 160×40 | Full | 點陣 logo、吉祥物、全部紋樣 |
| ≥ 120×30 | Standard | 半高 logo、吉祥物（空間夠時）、section motif |
| ≥ 90×22 | Minimal | 文字 logo、chip、mini strip |
| 更小 | None | 只有資料 |

丟掉的順序是固定的：**先裝飾，再視覺化樣式，資料留到最後**。
`decorations = "off"` 永遠贏；`"full"` 在畫面真的太小時仍然會退讓到 None ——
使用者設了 full 也不該讓版面炸掉。

### 頁面上做了什麼

| 頁面 | 改了什麼 |
|---|---|
| Overview | Highlights 卡片（最耗 CPU / 最滿掛載點 / 最忙介面 / GPU 閒置但 VRAM 佔著）+ 吉祥物槽 |
| CPU | per-core 熱度圖（`v` 切換，空間不足時自動），每列最多 16 顆並標出起始編號 |
| Memory | pattern 組成條 + 同紋理圖例 + 一句規則式判讀 |
| GPU | 硬體卡片：活躍的用強調色框，estimated 的標 `[ESTIMATED]` 並淡化 |
| Storage | pattern 使用率條、檔案系統與磁碟類型做成 chip |
| Network | 上下鏡像波形 —— 同一時刻的收與送上下相對 |
| Processes | CPU / MEM 的極短熱度條 + 斑馬紋（只換底色，不動任何顏色語義） |
| Admin | 麵包屑 `// ADMIN › User Storage › devuser`、專屬強調色、per-user waffle |

Highlights 的每一條都是**寫死的規則**加上算出來的數字，不是推測。
你可以自己去那一頁對照。

而且每一條都**指向它講的那個東西** —— 選到「最耗 CPU syncsvc (1322)」
按 `e`，看到的是那個行程的完整說明，不是一段泛泛的解釋。

### 為什麼 User Storage 用 waffle 而不是 treemap

treemap 要在整數格上切矩形，用量小的人會被捨入成 0 寬而整個消失 ——
在終端機這種粗格線的畫布上那是常態不是例外。

waffle 每格一樣大，而且**每個人至少分到一格**（先保底再按比例分配剩下的），
所以佔 0.1% 的人也看得到自己。那正是這個畫面存在的意義。

### 操作永遠優先於畫面

事件迴圈的順序是刻意的：**先把排隊的輸入吃完，再考慮要不要畫**，
而且畫之前會再確認一次沒有新的輸入進來。

反過來（先畫再收輸入）的話，動畫剛好到期而使用者同時按鍵時，
那一格動畫會先畫完、連同終端機的寫入一起花掉，按鍵才被看到。

三條規則：

1. **輸入造成的重畫不受畫格節流限制。** 節流器（25 fps）是為了不讓
   閒置的動畫燒 CPU，不是用來讓按鍵排隊的。事件在畫之前已經全部
   消化完，所以不受節流也不會失控重畫。
2. **使用者操作後 250 毫秒內，動畫完全讓位** —— 一格都不推進。
   按住方向鍵捲動時，吉祥物整段時間都停著。
3. **要畫之前再確認一次沒有新的輸入。** 有的話就回頭處理 ——
   那一幀反正也要被下一幀蓋掉。
4. **畫面上沒有吉祥物時，動畫時鐘整個停掉。** 八個頁面裡只有總覽有
   裝飾槽，小終端機更是完全放不下 —— 那些情況下跑動畫等於畫面上
   什麼都沒動卻每秒重畫三次。
5. **動畫不補幀。** 落後了就直接跳到現在該有的樣子，不重播中間那些。

實測（在有吉祥物動畫的頁面上，用隨機間隔按鍵去撞動畫到期的時刻）：

| | 中位數 | p90 | 最慢 |
|---|---|---|---|
| 修正前 | 11 ms | 21–255 ms | 25–262 ms |
| 修正後 | 11 ms | 25 ms | 25 ms |

重點不是中位數，是**長尾消失了**。修正前那個 262 毫秒的尾巴時有時無，
正是「有時候覺得卡住」的來源。

### 效能

實測，不是宣稱：

| 項目 | 成本 |
|---|---|
| 一幀（200×60，無裝飾） | 361 µs |
| 一幀（200×60，完整裝飾） | 410 µs |
| 光柵化一隻吉祥物 | 3.25 µs |
| 組一條 pattern bar | 173 ns |
| 推進一格動畫的額外成本 | 量不到 |

預設 3 fps 的動畫下，這大約是**一顆核心的 0.12%**。
上面每個數字都有對應的測試在抓回歸。

---


## 解釋系統（按 `e`）

這是 sysview 跟其他監控工具最大的差別，也是設計上最保守的一塊：

**說明不是即時生成的。** 沒有 LLM、沒有臨時開 shell 去查、沒有猜測。
每一段文字要嘛是編譯進二進位檔的結構化定義，要嘛是 collector 已經拿到的實測值。
同樣的機器狀態一定給出同樣的說明，所以它可以被測試 —— 也真的有測試在測。

按 `e` 會解釋兩種東西：

**固定 metric** —— `cpu.usage`、`mem.used`、`disk.usage` 這類。
定義寫在知識層裡：這是什麼、算式、資料來源、常見誤解、等價的原生指令。

**這台機器上的具體物件（entity）** —— 內容隨執行期而變，所以由 collector
的實測資料組出來：

| 選到什麼 | 說明會給你 |
|---|---|
| 一張 GPU | 廠商、型號、驅動版本、VRAM、時脈、功耗、compute 行程 |
| 一個網路介面 | 是實體還是虛擬、MAC、IP、連線速度、累計流量、錯誤與丟包 |
| 一個行程 | PID / PPID、使用者、狀態、CPU、RSS/VIRT、執行緒、命令列 |
| 一個掛載點 | 檔案系統類型、裝置、掛載選項、容量、inode |
| 一顆磁碟 | 型號、HDD 還是 SSD、容量、讀寫速率、IOPS、忙碌率 |
| 一個使用者 | uid、家目錄、shell、群組 |
| 一個路徑 | 作業系統提供的中繼資料，**不推測這個目錄拿來做什麼** |

說明太長時 `↑↓` 可以捲動它，捲到底就停住；其他任意鍵關掉。

說明的標題永遠是**你框住的那一格**，metric 的正式名稱與 id 放在副標。
選到 `Logical CPUs` 就會看到 `Logical CPUs`，不會看到 `CPU Utilization`。

每一格都標示**確定性**：`exact`（直接讀到的）、`derived`（算出來的）、
`estimated`（推估的，例如 Intel iGPU 使用率）、`unavailable`（拿不到，
不會拿 0 假裝有值）。GPU 的來源說明會依這台機器實際用的 backend 改寫，
所以它不會在沒有 NVML 的機器上宣稱自己在用 NVML。

**它不會描述自己不知道的東西。** 看到一個叫 `python` 的行程，它會告訴你
PID、使用者、吃多少記憶體，不會說「這是一個 PyTorch 訓練程式」。

隱私邊界跟監控頁面一致：普通使用者按 `e` 看到的，就是他本來就看得到的。
sysview 為了產生說明**也不會**去讀 `/proc/<pid>/environ` —— 環境變數裡
常有 token 跟密碼。

---


## 資料來源

sysview 刻意**不用** `sysinfo` 之類的抽象把來源蓋掉 —— 你能知道每個數字是從哪個檔案的哪一欄來的。

| 子系統 | 來源 |
|---|---|
| CPU | `/proc/stat`、`/proc/loadavg`、`/proc/cpuinfo`、`cpufreq` sysfs、`hwmon` |
| Memory | `/proc/meminfo` |
| Storage | `/proc/mounts`、`statvfs(2)`、`/proc/diskstats`、`/sys/block`、`hwmon` |
| Network | `/proc/net/dev`、`/sys/class/net`、`getifaddrs(3)`、`/proc/net/sockstat` |
| Process | `/proc/<pid>/{stat,cmdline}` |
| cgroup | `/sys/fs/cgroup/{cpu.max,memory.max,cpu.stat}`、`/proc/self/cgroup` |
| **NVIDIA** | **NVML（`libnvidia-ml.so`，runtime dlopen）** |
| Intel GPU | `/sys/class/drm/card*/`（RC6 殘留反推，標示為估計值） |
| AMD GPU | `/sys/class/drm/card*/device/gpu_busy_percent` 等 |

`nvidia-smi` 在 v2 只剩兩個角色：驅動壞掉時的診斷，以及 Explain 頁上
「同樣的數字用什麼原生指令可以看到」。它**不再是**正常輪詢的 backend。

---


## 效能

實測環境：i7-13700（24 執行緒）、62 GB RAM、RTX 4070 Ti、約 530 個行程。

| 項目 | 成本 |
|---|---|
| 單輪完整取樣 | **5.14 ms** |
| 1 Hz 下的 CPU | **0.51% 單核 = 全機 0.021%** |
| 每頁渲染 | 0.14 – 0.34 ms |
| 常駐記憶體（含 NVML） | ~21 MB |
| 常駐記憶體（`--no-gpu`） | **4.0 MB** |
| 執行檔 | sysview 2.3 MB · sysview-priv 0.8 MB |

各 collector 拆解：

| Collector | 單次成本 |
|---|---|
| Memory | 0.008 ms |
| Disk | 0.042 ms |
| Network | 0.107 ms |
| CPU（24 核 + 頻率 + 溫度） | 0.309 ms |
| GPU（NVML + sysfs） | 1.325 ms |
| **Process（掃全部 `/proc`）** | **3.345 ms** ← 65% 的成本 |

因此各 collector 有**獨立的取樣間隔**：行程清單最快 0.75 秒一次，
即使你把主更新率調到 0.2 秒。畫面則是**事件驅動**的，閒置時幾乎不耗 CPU。

### 記憶體的取捨

NVML 的 dlopen 會讓 RSS 增加約 15 MB —— **那是 NVIDIA 驅動函式庫自己的緩衝區，
不是 sysview 佔用的**。實測：

| 設定 | RSS |
|---|---|
| 無 collector、無 NVML | 2.5 MB |
| TUI，NVML 未載入 | 4.9 MB |
| TUI，NVML 載入 | 21.0 MB |

換來的是每次 GPU 輪詢從 fork `nvidia-smi` 的 **12.3 ms 降到 1.33 ms（約 10 倍）**。
在 server 上 CPU 通常比 15 MB 重要，但需要極小記憶體的部署可以用 `--no-gpu`。

sysview 也只在 `/sys/bus/pci/devices` 上真的有 NVIDIA 顯示卡時才做 dlopen ——
沒有 NVIDIA 的機器完全不付這個代價。

---


## 與其他工具的比較

| | sysview | btop | bottom | nvtop | Glances |
|---|---|---|---|---|---|
| 語言 | Rust | C++ | Rust | C | Python |
| CPU / 記憶體 / 磁碟 / 網路 / 行程 | ✓ | ✓ | ✓ | 部分 | ✓ |
| GPU | NVML + DRM sysfs | 有 | 有 | 專精 | 有 |
| **metric 說明 / 來源 / 算式 / 陷阱** | **✓** | — | — | — | — |
| **對應的原生指令教學** | **✓** | — | — | — | — |
| **GPU 驅動故障診斷** | **✓** | — | — | — | — |
| **各使用者儲存 / 記憶體 / GPU 分佈** | **✓（經 sudo）** | — | — | — | — |
| **精確值 vs 估計值標示** | **✓** | — | — | — | — |
| 遠端 / Web / REST API | — | — | — | — | ✓ |
| 容器原生支援 | 偵測 + 顯示配額 | 部分 | 部分 | — | ✓ |
| 行程管理（kill / renice） | 經 sudo helper，窄 allowlist | ✓ | ✓ | ✓ | ✓ |

**sysview 該用在什麼時候**：想搞懂系統在幹嘛、想教別人、要診斷 GPU 驅動、
要查多使用者機器上誰在吃資源。

**該用別的**：只是要一個漂亮的即時監控（btop 更成熟）、
需要遠端 / API（Glances）、只看 GPU（nvtop 更專精）。

---


## 無障礙

* **絕不只用顏色傳達嚴重程度。** 每一級都有專屬符號：`●` ok、`◐` notice、
  `▲` warning、`■` critical、`○` unknown。整個畫面轉成灰階仍然讀得懂。
* critical / warning 同時加粗，單色終端機下也分辨得出來
* 支援 `NO_COLOR` 標準
* 色深由終端機宣告的能力決定：`COLORTERM=truecolor` → 24-bit；`TERM=*-256color`（例如 macOS Terminal.app，它不送 `COLORTERM`）→ 256 色；其他 → 16 色。每一格都自己畫底色，淺色底的終端機也不會出現白框
* 自動偵測並降級：truecolor → 256 色 → 8 色 → 無色
* 中文寬度正確處理（全形字佔 2 格），框線不會歪
* 版面響應式降級：200×60 到 50×12 都能用，更小會顯示明確提示

---


## 已測試 / 未驗證

誠實區分：

### 已在實機驗證

| 項目 | 環境 |
|---|---|
| OS | Ubuntu 22.04.4 LTS |
| Kernel | 6.8.0-90-generic |
| libc | glibc 2.35 |
| CPU | Intel i7-13700（24 執行緒，coretemp） |
| NVIDIA | RTX 4070 Ti，driver 550.90.07，NVML |
| Intel GPU | UHD Graphics 770（i915，RC6 估計） |
| 儲存 | NVMe + 2× SATA HDD + USB，ext4 / vfat |
| 網路 | 有線 + 無線 |
| 終端機 | xterm-256color，190×48 到 20×5 |

解釋系統是在這台機器上逐頁走過的（PTY + pyte，讀回實際畫到終端機的字元）：
Overview / CPU / Memory / GPU / Storage / Network / Process 的固定 metric 與
runtime entity 都確認過標題、實測值、來源路徑與等價指令是真的 —— 包含
RTX 4070 Ti 的 NVML 讀值、`wlp0s20f3` 的 MAC 與 IPv4、`sda` 認出是 WDC
WD2003FZEX 機械硬碟（溫度誠實標成 unavailable，沒有捏造）、以及某個真實
PID 的完整狀態。

Admin 頁需要 sudo 密碼，沒辦法在自動化測試裡解鎖，所以它的導覽狀態機改用
**真的 renderer + 真的按鍵處理**跑整合測試（八個案例，涵蓋分頁切換、清單捲動
後的選取、Enter 展開的是游標所在的那個人）。表格內容本身仍屬於下面那一區。

### 已實作但**未經實機驗證**

* **AMD GPU（amdgpu）** —— 依 sysfs ABI 撰寫，開發機沒有 AMD 卡
* **多 GPU** —— 程式碼有處理，沒有多卡機器可測
* **容器內執行** —— cgroup v2 偵測與配額顯示已實作。CI 每次都會把執行檔
  丟進 `debian:12-slim` 跑 `--snapshot` / `--json`（另一個發行版、沒有 GPU、
  沒有 Rust、`/sys/class/hwmon` 被遮掉），確認不會 panic；但容器**內部**的
  cgroup 配額顯示還沒對著真的有限制的容器驗證過
* **非 x86_64** —— 沒有平台相依的組語，但沒測過 ARM
* **musl / Alpine** —— 沒測過
* **fahrenheit 溫度單位** —— 有實作，沒有人用過
* **Admin 頁解鎖後的表格內容** —— 導覽與版面有整合測試，但四個分頁真正的
  資料（各使用者的儲存 / 記憶體 / VRAM / socket）需要 sudo 才會產生，
  尚未在這台機器上實際解鎖驗證過

不會宣稱這些是 production verified。

---


## 已知限制

* 沒有 polkit 整合（只走 sudo）
* 啟動時的授權探測是非同步的，所以偶爾會先看到一瞬間的 `admin: locked` 才更新
* 容器裡的 CPU% 仍是相對於宿主機全部核心，還沒換算成相對於 cgroup 配額（UI 有標示）
* `--json` 沒有管理員模式（刻意：非互動輸出不做提權）
* 沒有遠端 / Web / REST API —— 這是單機工具
* 掃描進度只能靠經過時間估算，沒有串流進度回報

---


## 開發

```bash
cargo build --release        # 編譯
cargo test --all-features    # 五百多個測試
make lint                    # clippy -D warnings + fmt --check
make legacy                  # 編譯 C++ v1（regression oracle）
```

`sudo make install` 刻意**不會**重新編譯（sudo 下的 root 找不到你的 cargo，
而且以 root 編譯會在 `target/` 留下 root 擁有的檔案）。它改成核對
`make` 建出來時記下的 sha256 —— 因為 `cargo test --all-features` 會把
啟用 `test-support` 的版本覆蓋進 `target/release/`，沒有這道核對就會
把那個版本裝上去。對不上時它會直接拒絕並要你重跑 `make`。

（不能比時間：cargo 是用 hardlink 把 `deps/` 的產物接回 `target/release/`，
被換掉時 mtime 會跟著舊產物一起倒退。）


測試分佈：

| 類型 | 數量 | 內容 |
|---|---|---|
| 單元測試 | 353 | parser、格式化、CJK 寬度、去重、hard link、symlink、協定驗證 |
| helper 安全 | 16 | 參數驗證、注入防護、PID 重用、environ 洩漏 |
| 特權端到端 | 11 | 實際執行 helper binary，驗證它拒絕該拒絕的 |
| TUI 渲染 | 63 | 8 頁 × 11 種尺寸的框線與 CJK 對齊、Admin 導覽狀態機、裝飾降級 |
| Describe | 15 | 說明必須 deterministic、來源真實、不編造語意、不洩漏 |
| Parity | 9 | 對照 C++ v1、`df`、`ps`、`nvidia-smi`、`/proc/loadavg` |
| 效能 | 11 | 各 collector 成本、NVML vs fork、記憶體上限、裝飾與動畫的成本 |

（數字會隨版本變動，以 `cargo test` 為準。）

### 專案結構

```
src/
  main.rs                CLI、終端機生命週期、事件迴圈
  lib.rs                 模組宣告
  app.rs                 狀態機（按鍵 → 狀態，不畫圖也不讀檔）
  snapshot.rs            純文字與 JSON 輸出
  error.rs               Unavailable 分類

  collectors/            唯一會碰 /proc、/sys、NVML 的地方
    mod.rs               排程器（各 collector 獨立取樣間隔）
    cpu.rs  memory.rs  disk.rs  network.rs  process.rs
    thermal.rs           hwmon 共用探索
    cgroup.rs            cgroup v2 感知
    storage_scan.rs      du 語意的檔案系統走訪（有預算上限、可取消）
    util.rs              /proc 讀取、差分、速率換算
    gpu/
      mod.rs             三家統一模型
      nvidia.rs          NVML 主要 backend + 故障診斷
      intel.rs           i915 RC6 估計
      amd.rs             amdgpu sysfs

  metrics/
    model.rs             Reading（值 + 單位 + 品質 + 來源）、Series、Severity
    knowledge.rs         47 個固定 metric 的結構化定義（編譯進二進位檔）
    registry.rs          UI 選取項 → metric id
    describe.rs          DescribeTarget / EntityRef / DescribeContent
    describe/current.rs  把 collector 的實測值填進固定 metric 的說明
    describe/entity.rs   執行期物件的說明（GPU、介面、行程、掛載點、
                         磁碟、使用者、路徑、核心）

  privilege/
    protocol.rs          窄 allowlist、參數驗證
    client.rs            sudo 呼叫、helper 安全檢查
  bin/sysview-priv.rs    特權 helper（root，非 setuid）

  ui/                    只負責畫，絕不讀檔
    mod.rs               主繪製、頁首、頁尾、健康徽章
    layout.rs            響應式版面
    focus.rs             不限層數的空間焦點樹（方向鍵找最近的兄弟）
    visual/pattern.rs    七種紋理（是 encoding 不是裝飾）
    visual/glyph.rs      36 個 5×7 點陣 glyph
    visual/logo.rs       品牌 logo 的響應式降級
    visual/mascot.rs     狐狸與鹿，各五個姿態 + 動畫差異
    visual/motif.rs      虛線規、刻度、電路感紋樣
    visual/slot.rs       裝飾槽（唯一會把裝飾畫進 Buffer 的地方）
    widgets/viz.rs       heatmap / waffle / 鏡像波形 / mini strip / chip
    explain.rs           Explain 面板
    overview.rs cpu.rs memory.rs gpu.rs storage.rs network.rs
    process.rs admin.rs help.rs common.rs format.rs
    widgets/braille.rs   點陣面積圖（2×4 dots/格）
    widgets/gauge.rs     1/8 格細分的漸層長條
    widgets/panel.rs     圓角外框（CJK 安全）

  theme/mod.rs           7 個主題 + 色深降級
  config/mod.rs          設定檔（不能影響授權）

legacy/cpp/              C++17 v1，保留為 regression oracle
                         （含編好的執行檔：parity 測試直接拿它比對，
                          不必先裝 C++ 工具鏈）
```

Python 原型已經移除 —— 它沒有被任何東西引用，只是歷史紀錄。
C++ v1 留著是因為 `tests/parity.rs` 有 4 項比對以它為基準。
如果不再需要那個基準，把整個 `legacy/cpp` 拿掉就好，parity 會自己停用；
但只刪掉執行檔而留下原始碼會讓測試失敗，那是刻意的 —— 忘記 `make legacy`
的話那 4 項會靜靜跳過，測試看起來照樣是綠的，那比失敗還糟。

### 分層

```
collectors/   採樣、解析、差分、可用性判斷
     ↓
metrics/      正規化模型 + provenance + knowledge
     ↓
ui/           只 render
```

UI **絕不**出現 `read_to_string("/proc/...")`。

---

