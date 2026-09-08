# sysview

一個指令看完整台 Linux 機器，而且**看得懂每個數字是什麼**。

```bash
sysview
```

不需要 root。機器上每個使用者都能跑。

---

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

## 畫面

完整的實機截圖（每一頁、各種尺寸、有無裝飾）在 [`docs/visual-guide.txt`](docs/visual-guide.txt)。
全部吉祥物姿態在 [`docs/mascots.txt`](docs/mascots.txt)，
不同萃取方式的比較在 [`docs/mascot-styles.txt`](docs/mascot-styles.txt)。


```
▎sysview devlab  Ubuntu 22.04.4 LTS  Linux 6.8.0-90  up 21d 03:02      ● system healthy  03:24:41
  0 Overview   1 CPU   2 Memory   3 GPU   4 Storage   5 Network   6 Processes   A Admin ─────────
╭─┤ CPU ├─────────────────────────┤ 24 執行緒 ├╮╭─┤ Memory ├──────────────────┤ 62.5 GB ├╮
│  ███▉·························· ●    4.9%    ││ RAM  █████▎················ ●  5.41 GB / 62.5 GB │
│ 1.58 GHz · 58°C · load 1.37                  ││ SWP  ······················ ●  1.00 MB / 33.9 GB │
│                                        100   ││                                            100   │
│                          ⢀⣠⣴⣶⣤⡀        50   ││                          ⣤⣤⣤⣤⣤⣤⣤⣤⣤⣤       50   │
│  0 ▏···  1 ····  2 ····  3 ····  4 ████      ││ cache 34.6 GB · buffers 3.09 GB · avail 57.1 GB  │
╰──────────────────────────────────────────────╯╰──────────────────────────────────────────────────╯
╭─┤ GPU ├──────────────────────────────┤ 2 張 ├╮╭─┤ Storage ├────────┤ R 0 B/s  W 0 B/s ├╮
│ NVIDIA GeForce RTX 4070 Ti      39°C   4W    ││ /mnt/data    ████████████▎········ ●  1.39 TB free │
│ util   ························ ●     0.0%   ││ /            ███████████████████▏· ●   534 GB free │
│ vram   ························ ●  6.31 MB   ││ /mnt/scratch ···················· ●   436 GB free │
│ UHD Graphics 770 (Raptor Lake-S)             ││ /boot/efi    ▋··················· ●   505 MB free │
│ util~  ························ ●    ~0.0%   ││                                                    │
╰──────────────────────────────────────────────╯╰──────────────────────────────────────────────────╯
```

`util~` 的 `~` 代表**估計值** —— Intel 內顯沒有 `gpu_busy_percent`，
那是用 RC6 省電殘留時間反推的。sysview 不會假裝它是精確值。

---

## 安裝

### 環境需求

| | 需要什麼 | 沒有的話 |
|---|---|---|
| **編譯期** | Rust **≥ 1.88**（`rust-version` 就是這個值，CI 每次用剛好這版編一次驗證） | cargo 會直接說「請升級」，不會編到一半才爆 |
| | `cc` 連結器（gcc 或 clang）+ binutils | 連結階段失敗 `linker 'cc' not found`；相依樹裡沒有任何 C 程式碼要編，`cc` 只用在最後連結 |
| **執行期** | `libc` + `libgcc_s`，就這樣（`ldd` 只有這三行加 vdso） | — |
| | Linux。資料來源是 `/proc` 與 `/sys` | macOS / Windows 不支援，`install.sh` 會直接擋下來 |
| **選用** | `libnvidia-ml.so`（NVIDIA 驅動） | GPU 頁自動退回 sysfs / 不顯示 NVIDIA；**執行檔沒有連進 NVIDIA 函式庫**，是執行期 dlopen |
| | `sudo` + root 安裝的 helper | Admin 頁顯示 `helper not installed`，其餘七頁完全不受影響 |
| | `g++`（只有 `make legacy` 用，編 v1 的 C++ 對照程式給 parity 測試） | 一般使用者與安裝流程都不需要；沒有它 parity 比對會自動跳過 |

Rust 用 rustup 裝在家目錄，**不需要 root**：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 兩種安裝方式

```bash
./install.sh          # 編譯 + sudo 安裝到 /usr/local（全機共用，含 Admin 頁）
./install.sh --user   # 編譯 + 裝到 ~/.local（只給自己，完全不需要 root）
```

或直接用 make：

```bash
make && sudo make install    # 全機共用
make && make install-user    # 只給自己，不需要 root
```

**在沒有 sudo 的機器上**（例如共用的實驗室主機）：`--user` 這條路你自己就能
走完，七個一般頁面全部可用。只有 Admin 頁需要管理員跑一次
`sudo make install` —— 那是因為它要一個 **root 擁有**的 helper：
`locate_helper()` 只認寫死的系統路徑而且不接受環境變數覆寫，家目錄裡的
helper 永遠不會被採用。這是刻意的，讓 sudo 去執行一個你自己寫得動的檔案
等於把 root 送出去。

安裝內容（`sudo make install`）：

```
/usr/local/bin/sysview                     一般使用者執行，不需要 root
/usr/local/libexec/sysview/sysview-priv    root:root 0755，**不帶 setuid**
/usr/local/share/doc/sysview/              README、安全文件、第三方聲明
/var/lib/sysview/dino/                     1777（sticky，同 /tmp）：恐龍彩蛋的全機排行榜，
                                           每人只能寫自己的 <uid>.json（見 docs/dino.md）
```

`make install-user`（不需要 root）：

```
~/.local/bin/sysview                       只給自己；Admin 頁不會啟用（沒有 root 擁有的 helper）
~/.local/share/doc/sysview/                文件
~/.local/share/sysview/dino-scores.json    彩蛋排行榜的私人退路 —— 機器上沒有 /var/lib/sysview/dino 時用；
                                           管理員裝過一次系統版之後，--user 裝的也會自動改用全機共用榜
```

執行期讀的東西都是每台機器自己的：`/proc`、`/sys`、目前帳號（`getpwuid`）、
`~/.config/sysview/config.toml`（沒有就用預設）。沒有任何寫死的主機名、路徑或帳號。

原始碼也可以一起放到共用位置，讓其他管理員能維護、重編：

```bash
sudo make install-source                       # → /usr/local/src/sysview
sudo make install-source SRCGROUP=sysadmins    # 順便讓某個群組可寫
```

安裝器**不會修改你的 sudo 政策**，不會寫 `/etc/sudoers.d`，不會建立 NOPASSWD 規則。

### 為什麼要 sudo？裝完之後每個帳號都能用嗎？

**sudo 只用在「寫入 `/usr/local`」這一步**，因為那個目錄是 root 的。
裝完之後就跟 sudo 無關了：

```
/                       drwxr-xr-x root:root
/usr                    drwxr-xr-x root:root
/usr/local              drwxr-xr-x root:root
/usr/local/bin          drwxr-xr-x root:root
/usr/local/bin/sysview  -rwxr-xr-x root:root   ← 所有人可執行
```

`/usr/local/bin` 本來就在 Ubuntu 預設的 PATH 上（`/etc/environment`），
所以**任何帳號登入後直接打 `sysview` 就能用**。

執行期只需要 `libc` 與 `libgcc` —— 其他使用者**不需要**安裝 Rust，
Rust 只有「編譯」那一次才用得到。

`make install` 會自動驗證這一點：它會走過整條路徑檢查每一層的權限，
只要有任何一層擋住其他使用者就會直接失敗，不會裝完才發現別人用不了。

### 沒有 sudo 權限的使用者怎麼辦？

**照樣能用，而且是全部的監控功能。** 七個一般頁面
（總覽 / CPU / 記憶體 / GPU / 儲存 / 網路 / 行程）完全不需要任何特權 ——
資料來源就是 `/proc` 與 `/sys`，那些本來就是所有人可讀的。

唯一用不到的是 **Admin 頁**，因為那裡顯示的是**其他使用者的**資料
（誰佔了多少磁碟、誰吃了多少記憶體、誰在用 GPU）。沒有權限的人會看到：

```
你沒有執行管理員功能的權限。

這不是錯誤 —— 是系統的 sudo 政策正確地拒絕了這個請求。
若你認為應該有權限，請聯絡管理員檢查 sudoers 設定。
```

不是崩潰、不是空白畫面，也不會顯示任何不該看到的資料。

---

## 鍵盤

導航像螢幕的 OSD 選單：**一層一層進去，一個鍵退回來**。層數不設限。

```
分頁層        ← →  切換分頁              ↓ / Enter  進入這一頁
   ↓ Enter                                    ↑ Esc
第 1 層 面板   ← ↑ ↓ →  在面板之間移動    Enter  往下鑽
   ↓ Enter                                    ↑ Esc
第 2 層 每一列  ← ↑ ↓ →  在列之間移動     Enter  再往下鑽
   ↓ …                                        ↑ …
```

方向鍵永遠只在**同一層的兄弟之間**移動，而且依照畫面上的實際相對位置，
所以不管鑽到多深，上下左右的意思都不會變。

被選中的那一格會直接**框起來**，畫面最下面一列顯示麵包屑：
`Core Statistics › Avg Frequency   3/12   第 2 層`。

面板用外框加左上角的 `▣`；面板裡的**一列**沒有自己的框線，所以只上色、
記號 `▸` 放在旁邊的空白欄 —— 記號絕對不會蓋掉那一列的內容。

| 鍵 | 作用 |
|---|---|
| `← ↑ ↓ →` | 分頁層是切換分頁；進入後是在同一層移動 |
| `Enter` | 往下鑽一層 |
| `Esc` | 回到上一層 |
| **`e`** | **解釋框住的那一格**（意義 / 來源 / 算式 / 陷阱 / 原生指令）|
| `PgUp` `PgDn` | 在清單裡大幅捲動 |
| `0` | 總覽（`` ` `` 與 `~` 也可以） |
| `1`–`6` | CPU / 記憶體 / GPU / 儲存 / 網路 / 行程 |
| `A` | 管理員擴充 |
| `Tab` `Shift-Tab` | 依序切換頁面（任何一層都有效）|
| `空白` | 暫停 / 繼續取樣 |
| `+` `-` | 更新頻率 0.2s – 10s |
| `t` | 切換主題 |
| `m` | 切換吉祥物：狐狸 → 鹿 → 關閉（不寫回設定檔）|
| `u` | 透過 sudo 解鎖管理員功能 |
| `s` `/` | （行程頁）換排序 / 關鍵字篩選 |
| `q` | 離開 |

鑽到最裡層可以選到**單一數字** —— `Logical CPUs`、`Avg Frequency`、
`Core clock`、單一掛載點、單一行程，每一個都能按 `e` 看說明。
選到單一行程這一層，也是未來「停掉這個行程」之類操作要掛的地方。

### Admin 頁的導航

Admin 頁的分頁列**本身就是焦點樹的第一層**，所以不需要記特別的鍵：

```
Admin 頁      Enter  →  分頁列（← → 在 User Storage / Memory / GPU / Sockets 之間切）
                 Enter  →  使用者清單（↑ ↓ 逐一選人）
                    Enter  →  展開游標所在使用者的儲存明細
                       Esc  ←  一次退一層
```

`[` 與 `]` 保留成捷徑，讓你不進分頁列也能直接換分頁，但已經不是必要的操作。

四個分頁的清單都會捲動，選到的那一列一定留在畫面上；選取狀態跟著**清單裡的
絕對位置**走，不是「畫面上第幾列」，所以捲動之後選取不會跳掉。

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

## 命令列

```bash
sysview                     互動儀表板
sysview -i 0.5              更新間隔 0.5 秒
sysview -v gpu              直接開 GPU 頁
sysview --theme nord        指定主題
sysview --no-gpu            不載入 NVML，常駐記憶體從 20 MB 降到 4 MB
sysview --no-splash         跳過啟動品牌畫面（預設會顯示，1.5 秒後自己消失）
sysview --mascot deer       這一次用鹿（不寫回設定檔）

sysview --snapshot          純文字快照後離開（沒有 TTY 時自動採用）
sysview --json | jq .       機器可讀輸出
sysview --explain cpu.load  在終端機直接查一個 metric，不進 TUI
sysview --list-metrics      列出全部 47 個可解釋的 metric
sysview --print-config      印出帶註解的設定檔範本
```

適合 `watch`、`cron`、pipe：

```bash
watch -n5 'sysview --snapshot'
sysview --json | jq '.storage.mounts[] | select(.usage_percent.value > 80)'
*/5 * * * * /usr/local/bin/sysview --json --no-gpu >> /var/log/sysview.jsonl
```

---

## 頁面與 metric

### CPU
總使用率、每核使用率與頻率、每核溫度、load 1/5/15、每核 load、
iowait、steal、context switch/s、中斷/s、新行程/s、running/blocked、
開機時間、最耗 CPU 的行程、cgroup CPU 配額

### Memory
總量 / 已用 / 可用、page cache、buffers、swap、
**組成長條**（used / buffers / cache / free 的比例）、
完整 `/proc/meminfo`、最耗記憶體的行程、cgroup 記憶體配額

### GPU
使用率、VRAM、溫度、功耗與上限、核心/記憶體時脈、風扇、
效能狀態（P0–P15）、PCIe link、compute 行程、多 GPU、
驅動不可用時的完整診斷、精確值 vs 估計值標示

### Storage
掛載點（容量 / 已用 / 可用 / 使用率 / inode）、
每個裝置的讀寫吞吐量、IOPS、忙碌率、NVMe 溫度、
bind mount 去重、**使用率與 `df` 完全一致**

### Network
每個介面的狀態、速度、MAC、IP、
即時與歷史吞吐量、錯誤 / 丟包、socket 統計

### Processes
PID / PPID / USER / 狀態 / CPU% / MEM% / RSS / VIRT / 執行緒 / CPU 時間 / 完整命令列，
可排序（5 種）、可篩選、可捲動

### Admin（需 sudo）
各使用者的家目錄用量與逐層明細、
各使用者的記憶體（**含 PSS**，比 RSS 公平）、
各使用者的 GPU VRAM、
socket → 行程 → 使用者 對應

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

## 管理員擴充與 sudo

```
┌───────────────────────────────┐
│ sysview                       │  你自己的身分，永遠不是 root
└──────────────┬────────────────┘
               │  argv（沒有 shell、沒有路徑、沒有指令字串）
               ▼
          ┌─────────┐
          │  sudo   │  ← sudoers / PAM / LDAP 決定准不准
          └────┬────┘
               ▼
┌───────────────────────────────┐
│ sysview-priv                  │  root，但**不是 setuid**
│ 做完一件 allowlist 上的事就結束 │
└───────────────────────────────┘
```

三條紅線：

1. **整支 TUI 絕不以 root 執行。** `sudo sysview` 不是建議用法（會顯示警告）。
2. **helper 絕不 setuid。** 提權只能經過 sudo，授權才會落在 sudoers 手上。
3. **授權由 sudo 決定，不由我們決定。** 不看使用者在不在 `sudo` 群組 ——
   那在 LDAP / AD / 自訂 sudoers 的環境會判斷錯。

**sysview 不碰你的密碼。** 按 `u` 時它會完整還原終端機、把畫面交給 sudo，
由 sudo 自己處理密碼提示 / PAM / 指紋 / 硬體金鑰。

詳見 [docs/sudo.md](docs/sudo.md)。

---

## 隱私

* **絕不讀 `/proc/<pid>/environ`** —— 那裡面幾乎一定有 token 與密碼
* **不寫任何暫存檔、快取或 log** —— 管理員資料的生命週期就是 TUI 的記憶體
* **不做封包擷取** —— 網路只看中繼資料，不看內容
* **不做 DNS 反解**
* 非特權模式完全遵守 OS 權限，沒有任何提權捷徑
* `--json` **不會**繞過權限：資料來源與互動模式完全相同

完整的 Confidentiality / Integrity / Availability 分析見
[docs/security.md](docs/security.md)。

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

## 設定

`~/.config/sysview/config.toml`（用 `sysview --print-config` 產生範本）：

```toml
interval = 1.0            # 0.2 – 60 秒
theme = "default"         # default / high-contrast / catppuccin
                          # tokyo-night / nord / gruvbox / dracula
default_page = "overview"
history = 240             # 30 – 4096
temperature_unit = "celsius"
graph_style = "area"
show_per_core = true
process_sort = "cpu"
gpu = true                # false 可省約 15 MB

[branding]
name = "DEVLAB"           # A–Z 0–9，最多 6 字；留空就用 sysview 自己的名字
logo = "auto"             # auto / pixel / small / text / off
mascot = "fox"            # fox / deer / auto / none
mascot_mode = "reactive"  # reactive / manual / idle
mascot_state = "observe"  # manual 模式的固定姿態
mascot_fps = 3            # 1 – 8
splash = true             # 1.5 秒後自己消失，任何鍵跳過

[visual]
pattern = "braille"       # solid / shade / dot / braille / line / hatch / digital
density = "auto"          # auto / comfortable / compact
decorations = "auto"      # off / auto / full

[labels]
cpu = "BRAIN"             # 顯示成 "BRAIN · CPU"，不是取代
gpu = "CUDA"
memory = "MEM"
storage = "VAULT"
network = "LINK"
```

品牌代號與別名都收窄成 **A–Z 0–9、最多 6 個字、自動轉大寫**。
這麼窄是因為它們會排進頁首與面板標題，全形字會把框線撐破 ——
那是 v1 實際壞過的方式。不合法的值會退回預設並顯示提示，
不會讓版面爆掉。

**別名是加上去的，不是取代。** `BRAIN · CPU` ——
換了機器、換了人看，還是知道那是什麼。窄畫面時別名讓位給原名。

設定檔**不能影響授權**。裡面沒有 `admin = true` 這種欄位，
而且用 `deny_unknown_fields` 解析 —— 手動加了會直接報錯，
不會讓人誤以為自己開了什麼開關。

不合理的值會被夾回合法範圍並顯示提示（例如 `interval = 0.001`
會被夾成 0.2，避免 sysview 自己變成負載來源）。

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
cargo test --all-targets     # 299 個測試
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

合計 478 個，全數通過。

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

## 授權

MIT

## 授權

MIT（見 `LICENSE`）。恐龍彩蛋的剪影衍生自 Chromium 的像素畫，保留其
BSD-3-Clause 聲明，見 `THIRD_PARTY_NOTICES.md`。

