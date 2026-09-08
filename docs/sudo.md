# sysview 的 sudo 與授權

## 一句話

sysview 沒有自己的權限系統。**能不能用管理員功能，完全由你系統既有的 sudo 政策決定。**

---

## 為什麼不自己做一套

一個監控工具如果自己維護「誰是管理員」，就會出現三個問題：

1. 多了一份要維護、要稽核、可能過期的授權來源；
2. 它跟系統真正的授權（sudoers / PAM / LDAP / AD）可能不一致；
3. 只要有人能編輯那個設定檔，就等於能提權。

所以 sysview 刻意不做：

* 沒有 sysview 專屬的管理員帳號
* 沒有 sysview 自己的密碼
* 設定檔裡**沒有** `admin = true` 這種東西（有人加了會直接被 `deny_unknown_fields` 擋掉）
* 不用「使用者在不在 `sudo` / `wheel` 群組」來做判斷 —— 那在 LDAP、AD、自訂 sudoers 的環境會判斷錯

唯一的授權判準是：**`sudo` 讓不讓你執行 `sysview-priv`**。

---

## 架構

```
┌───────────────────────────────┐
│ sysview                       │  你自己的身分執行
│ 非特權 TUI                    │  永遠不是 root
└──────────────┬────────────────┘
               │  argv（沒有 shell、沒有路徑、沒有指令字串）
               ▼
          ┌─────────┐
          │  sudo   │  ← sudoers / PAM / LDAP 在這裡決定准不准
          └────┬────┘
               ▼
┌───────────────────────────────┐
│ sysview-priv                  │  root，但**不是 setuid**
│ 最小化 helper，做完一件事就結束 │
└───────────────────────────────┘
```

### 為什麼 helper 不是 setuid

setuid 會讓**任何人**都能以 root 執行它，授權判斷就落到 helper 自己身上。
那等於重蹈上面說的覆轍，而且 setuid 程式的攻擊面（環境變數、
`LD_PRELOAD`、resource limit、檔案描述子繼承）遠比一般程式大。

走 sudo 的話：

* 授權決定留在 sudoers（可稽核、可集中管理、可用 LDAP）
* sudo 會清理環境變數
* sudo 自己會留下 log

安裝後可以自己確認：

```bash
ls -l /usr/local/libexec/sysview/sysview-priv
# -rwxr-xr-x 1 root root ...    ← 沒有 s
```

`make install` 與 `install.sh` 都會自動檢查這一點，裝錯會直接失敗。

---

## 預設行為

**安裝器不會碰你的 sudo 設定。** 它不寫 `/etc/sudoers`，也不在
`/etc/sudoers.d/` 放任何東西。

所以預設情況下：

* 本來就有 sudo 權限的人 → 按 `u`，輸入自己的密碼，就能用管理員功能
* 本來沒有 sudo 權限的人 → 會看到 `not authorized`，這是**正確的拒絕**，不是 bug

---

## 可選：收緊權限

如果你想讓某些人**只能**執行 sysview 的 helper、不能執行其他 root 指令，
可以自己加一條規則。以下都是**可選的**，sysview 不需要它們也能運作。

### 只允許特定群組執行 helper

```
# /etc/sudoers.d/sysview
# 用 visudo -f /etc/sudoers.d/sysview 編輯，不要直接用文字編輯器

Cmnd_Alias SYSVIEW_PRIV = /usr/local/libexec/sysview/sysview-priv

%sysview-admins ALL=(root) SYSVIEW_PRIV
```

這樣 `sysview-admins` 群組的成員可以用管理員功能（仍需輸入密碼），
但不會因此獲得其他 root 權限。

### 唯讀：不允許送訊號與 renice

helper 的九個操作裡只有兩個會改變系統狀態
（`process-signal` 與 `renice`）。要禁止它們：

```
Cmnd_Alias SYSVIEW_RO = \
    /usr/local/libexec/sysview/sysview-priv capability, \
    /usr/local/libexec/sysview/sysview-priv storage-users, \
    /usr/local/libexec/sysview/sysview-priv storage-user-detail *, \
    /usr/local/libexec/sysview/sysview-priv user-memory, \
    /usr/local/libexec/sysview/sysview-priv gpu-users, \
    /usr/local/libexec/sysview/sysview-priv socket-map, \
    /usr/local/libexec/sysview/sysview-priv process-detail *

%operators ALL=(root) SYSVIEW_RO
```

sysview 會把被拒絕的操作顯示成 `not authorized`，不會 crash。

---

## 不要做的事

### 不要設 NOPASSWD

```
# 不要這樣做
%users ALL=(root) NOPASSWD: /usr/local/libexec/sysview/sysview-priv
```

這會讓**任何**能在該機器上執行程式的人（包含被入侵的網頁程式、
被偷到的 SSH 金鑰）不需要任何驗證就能：

* 讀取所有使用者的家目錄結構與用量
* 讀取所有行程的細節
* 對行程送出訊號

sysview 的功能不值得換掉這層防護。輸入密碼只有一次，
之後 sudo 的憑證快取（預設 15 分鐘）會讓你不必重複輸入。

### 絕對不要設成任意指令

```
# 極度危險，絕對不要
%users ALL=(ALL) NOPASSWD: ALL
```

安裝器不會產生這種設定，也不該有人手動加。

---

## 密碼怎麼處理

**sysview 完全不碰你的密碼。**

按 `u` 解鎖時，sysview 會：

1. 完整還原終端機（離開 alternate screen、關掉 raw mode）
2. 執行 `sudo -- <helper> capability`，把終端機交給 sudo
3. 等 sudo 結束，看它的離開碼
4. 重新進入 TUI

密碼提示、PAM、指紋辨識、硬體金鑰 —— 全部由 sudo 自己處理。
sysview 沒有讀密碼的程式碼，也沒有存密碼的地方。

平時的查詢用 `sudo -n`（非互動），保證**不會**在 TUI 還在畫面上時
突然跳出密碼提示把畫面弄壞。

---

## 稽核

會改變系統狀態的操作（送訊號、renice）會寫進 syslog / journald：

```bash
journalctl -t sysview-priv
```

```
sysview-priv[12345]: uid=1000 op=process-signal target_pid=4321 signal=TERM result=ok
```

只記中繼資料：誰做的、對誰做、做了什麼、結果。
**不記密碼、不記環境變數、不記完整命令列** —— 那些可能含機密。

`sudo` 自己也會留下 log，兩者可以互相佐證。

---

## helper 支援的全部操作

這就是完整的 allowlist。沒有 `exec`、沒有 `run`、沒有 `sh -c`。

| 操作 | 會改狀態 | 說明 |
|---|---|---|
| `capability` | 否 | 探測 helper 版本與權限 |
| `storage-users` | 否 | 各使用者家目錄用量 |
| `storage-user-detail --user X` | 否 | 單一使用者的第一層明細 |
| `user-memory` | 否 | 各使用者記憶體（RSS / PSS） |
| `gpu-users` | 否 | 各使用者 GPU VRAM |
| `socket-map` | 否 | socket → 行程 → 使用者 |
| `process-detail --pid N` | 否 | 行程細節（**不含 environ**） |
| `process-signal --pid N --starttime T --signal S` | **是** | 只接受 TERM / INT / HUP |
| `renice --pid N --starttime T --nice V` | **是** | 只接受 0–19（不能提高優先度） |

注意：

* **沒有任何操作接受路徑。** 家目錄是 helper 自己查 password database 得到的，
  所以路徑穿越在架構上就不可能發生。
* **訊號是 allowlist，不接受數字。** 沒有 SIGKILL —— 那會讓行程沒機會清理。
* **改狀態的操作必須帶 `starttime`。** helper 執行前會重新比對
  `(pid, starttime)`，避免 PID 被重用時打到錯的行程。
