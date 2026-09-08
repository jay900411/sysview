# legacy — sysview v1

保留為 **Rust v2 的 behavioral reference / regression oracle**，不是垃圾。

* `cpp/`    — C++17 實作（2810 行），零外部相依，自己寫 termios + ANSI 雙緩衝差分渲染
* `README.v1.md` — v1 當時的文件，含實測數字與設計決定

## 還是可以照舊使用

```bash
make legacy          # 從專案根目錄
cd legacy/cpp && ./sysview
```

## v2 怎麼用它

`tests/parity.rs` 會實際執行 `legacy/cpp/sysview --snapshot`，比對：

* 邏輯核心數必須完全一致
* 記憶體總量必須一致
* 掛載點清單與數量必須一致（去重邏輯相同）
* 磁碟使用率必須符合 `df` 的定義

沒編譯 C++ 版時這些測試會自動略過（CI 上不強制）。

## v1 → v2 的主要差異

| | v1 (C++) | v2 (Rust) |
|---|---|---|
| NVIDIA backend | fork `nvidia-smi`（12.3 ms/次） | NVML 直接呼叫（1.33 ms/次） |
| 架構 | 單檔 2810 行 | 分層模組，collector / metric / ui 分離 |
| metric 說明 | 寫死在 UI 字串裡 | 結構化知識層，39 個 metric，按 `e` 查 |
| 管理員功能 | 無 | sudo + 最小化 helper |
| 主題 | 固定 | 7 個，含色深降級 |
| 測試 | 手動 PTY 截圖 | 299 個自動測試 |
