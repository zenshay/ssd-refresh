# ssd-refresh

檔案層級的 SSD 刷新工具。它找出「讀起來特別慢的舊資料」，再請檔案系統把這些檔案搬到新的位置。搬移等於重寫一次，讀取速度就會恢復。

- 掃描只讀，不寫入任何東西。
- 刷新由檔案系統執行搬移。正在使用中的檔案也能安全處理。
- 程式不直接寫入磁區。

## 為什麼需要它

NAND 快閃記憶體的電荷會隨時間流失。很久以前寫入、之後一直沒有改過的資料，SSD 要反覆重試、加強錯誤校正才讀得出來，速度就掉下來。

實例：一顆用了 5.4 年的 Crucial BX500 480 GB，讀多年前寫入的舊資料只有 5–35 MB/s。把同一份資料複製一次，新的複本讀取約 500 MB/s。

DiskFresh 之類的工具用「讀出磁區再寫回」的方式刷新整顆磁碟，有兩個問題：

1. 系統碟上的檔案隨時在變。工具讀出一個磁區後，程式改了它，工具再寫回舊內容，就把新資料蓋掉了。
2. 它連空白區域也寫，浪費 SSD 的寫入壽命，之後還要再做一次 TRIM。

ssd-refresh 改成檔案層級：只處理讀得慢的檔案，而且用檔案系統的重組 API 搬移。檔案系統會和其他程式的讀寫同步，所以不會發生上面的競態。

## 支援的平台

| 平台 | 檔案系統 | 掃描 | 刷新 |
|---|---|---|---|
| Windows 10/11 | NTFS | 支援 | 支援（`FSCTL_MOVE_FILE`） |
| Linux | ext4 | 支援 | 支援（`EXT4_IOC_MOVE_EXT`），尚未實機驗證 |
| Linux | btrfs、XFS、其他 | 支援 | 不支援 |
| macOS | APFS | 支援（不分 extent） | 不支援 |

macOS 不支援刷新的原因：APFS 沒有能安全搬移使用中檔案的 API。

## 建置

需要 Rust 1.83 以上。

```bash
cargo build --release
```

執行檔在 `target/release/ssd-refresh`（Windows 是 `.exe`），約 600 KB，不需要額外的執行階段。`refresh-test` 是測試用的輔助程式。

## 用法

### 1. 掃描（只讀，不需要系統管理員）

```bash
ssd-refresh scan C:\ --out C-scan.csv
```

- 程式會逐檔讀取，不經過系統快取，並記錄每個 extent 的讀取速度。
- 低於 `--threshold`（預設 100 MB/s）的檔案標成慢。
- 以系統管理員執行時，可以讀到權限受限的檔案。一般權限讀不到的檔案會記成 `ERROR`。
- 加 `--hash` 會順便算每個檔案的 SHA256。

### 2. 刷新（需要系統管理員／root）

先用 `--dry-run` 看看哪些檔案會被處理。這一步只重新測速，不搬移：

```bash
ssd-refresh refresh --from C-scan.csv --dry-run
```

確定後才正式刷新。程式會列出摘要，要求輸入磁碟代號確認：

```bash
ssd-refresh refresh --from C-scan.csv --allow-system-volume
```

- 刷新系統磁碟必須另外加 `--allow-system-volume`。
- 預設只處理報告裡標成慢的檔案（`--select slow`）。刷新前程式會再測一次速，已經不慢的檔案會跳過。`--force` 會連快的也搬。
- 做完後執行 TRIM，讓 SSD 回收舊位置：Windows 用 `Optimize-Volume -DriveLetter C -ReTrim`，Linux 用 `sudo fstrim -v /`。

### 暫停、停止與續跑

- 在終端機輸入 `p` 再按 Enter：暫停或繼續。
- 輸入 `q` 再按 Enter，或按 Ctrl+C：做完目前的檔案後停止。再按一次 Ctrl+C 會立刻結束。立刻結束也不會損壞檔案，因為每一次搬移都是檔案系統的原子操作。
- 建立「輸出檔名.pause」檔案也會暫停，刪掉就繼續。這個方式適合腳本控制。
- 用同樣的指令加 `--resume` 可以從中斷處續跑。輸出的 CSV 每寫一列就存檔，它本身就是檢查點。

### 參數

| 參數 | 預設 | 說明 |
|---|---|---|
| `--threshold` | 100 | 低於多少 MB/s 算慢（MB = 10^6 位元組） |
| `--allowance-ms` | 2 | 每個 extent 容許的額外延遲 |
| `--chunk-kb` | 1024 | 每個 I/O 的大小 |
| `--qd` | 4 | 同時排隊的 I/O 數 |
| `--priority` | verylow | 大量讀取與搬移的 I/O 優先權 |
| `--measure-priority` | normal | 確認重讀與刷新前後測速的優先權 |
| `--max-chunk-mb` | 16 | 刷新時每次搬移的最大長度 |

`--allowance-ms` 的作用：小檔的讀取時間主要是固定延遲。例如 4 KB 讀 0.1 ms，換算只有 40 MB/s。程式判斷慢的條件是「時間 > 大小 ÷ 門檻 + 容許延遲」，小檔就不會被誤判。

## 輸出

### 掃描報告的狀態（`status` 欄）

| 狀態 | 意思 |
|---|---|
| `OK` | 已測速。`slow` = 1 表示慢 |
| `SKIP` | 不測速的種類，見 `kind` 欄 |
| `DUP_LINK` | 硬連結，同一個檔案已經掃過 |
| `ERROR` | 開不了或讀不了，原因在 `error` 欄 |

`kind` 欄的值：`normal`、`sparse`（稀疏檔）、`encrypted`（EFS 加密）可以刷新。`resident`（資料放在 MFT 或 inode 裡的小檔）、`compressed`（NTFS 壓縮）、`wof`（Windows 系統壓縮檔）、`shared`（reflink 共用）、`reparse`、`empty` 不刷新。

### 刷新記錄的狀態

| 狀態 | 意思 |
|---|---|
| `OK` | 搬移完成，前後雜湊一致 |
| `CHANGED` | 搬移期間有其他程式修改檔案，雜湊不同屬於正常 |
| `SKIP_FAST` | 重新測速後已經不慢，沒有搬 |
| `SKIP_KIND` | 不支援的種類 |
| `MOVE_FAILED` | 檔案系統拒絕搬移；檔案內容仍完整 |
| `PARTIAL` | 被停止，只搬了一部分；續跑時會重做 |
| `MISMATCH` | 雜湊不同，而且沒有偵測到其他程式寫入。程式會立刻停止 |
| `ERROR` | 其他錯誤 |

`before_mbps`、`after_mbps` 是搬移前後的讀取速度。

## 原理

### 掃描

1. 列出資料夾底下的檔案，不進入符號連結、junction 和掛載點，也不跨到別的磁碟區。
2. 查出每個檔案的 extent。Windows 用 `FSCTL_GET_RETRIEVAL_POINTERS`，Linux 用 `FIEMAP`。
3. 不經系統快取讀取。Windows 用 `FILE_FLAG_NO_BUFFERING`，Linux 用 `O_DIRECT`，macOS 用 `F_NOCACHE`。每次同時排 4 個 1 MB 的讀取：Windows 用 overlapped I/O，Unix 用多條執行緒的 `pread`。
4. 依完成時間算出每個 extent 的讀取時間。
5. 第一次讀起來慢的檔案先記下來。整輪掃完後用 normal 優先權確認一次，至少 30 秒後再確認一次。三次讀取取最快的結果，仍然慢才算慢。

### 刷新（Windows NTFS）

1. 讀整個磁碟區的叢集點陣圖（`FSCTL_GET_VOLUME_BITMAP`），排除 MFT 保留區。
2. 不經快取讀一次檔案，記下 SHA256 和速度。
3. 依邏輯位置順序，每次最多 16 MB，用 `FSCTL_MOVE_FILE` 搬到空的叢集。目標位置被別人搶先用掉時，程式重新讀點陣圖再試。
4. 再讀一次檔案，比對 SHA256，並記錄新的速度。

`FSCTL_MOVE_FILE` 是 Windows 磁碟重組工具用的 API。NTFS 會鎖住該範圍、複製資料、更新配置，整個過程對其他程式的讀寫是一致的。

### 刷新（Linux ext4）

程式在同一個檔案系統建立一個暫存的「捐贈檔」（`O_TMPFILE`），先預先配置空間，再用 `EXT4_IOC_MOVE_EXT` 交換資料區塊。這和 `e4defrag` 用的是同一個 API，核心會和其他程式的讀寫同步。

### 怎麼判斷雜湊不同是正常的

搬移前後雜湊不同時，程式會檢查兩件事：

- 檔案的大小、修改時間、變更時間、USN（NTFS）有沒有變。
- 有沒有別的程式開著寫入。Windows 用「不允許別人寫入」的共用模式開檔一瞬間測試。Linux 用讀取租約（`F_SETLEASE`）測試。

有任何一項成立，就記成 `CHANGED`。都不成立就記成 `MISMATCH`，程式立刻停止。

## 限制

- 刷新只支援 Windows NTFS 和 Linux ext4。
- 不處理 NTFS 壓縮檔、WOF 壓縮的系統檔、MFT 裡的小檔、檔案系統的中繼資料、資料夾索引和替代資料流。
- `pagefile.sys`、`hiberfil.sys` 這類被系統獨占的檔案打不開，會記成 `ERROR`。
- 速度判斷是統計結果，會受其他程式的 I/O 影響。詳見下一節。

## 測速的陷阱（實測發現）

1. **VeryLow 優先權會讓測速失真。** 同一批檔案用 normal 優先權讀幾乎都超過 100 MB/s，用 verylow 讀卻有 10–80 MB/s 的假性慢檔。所以程式用 verylow 做第一輪，用 normal 確認。
2. **VeryLow 的讀取可能被餓死。** 其他程式大量寫入同一顆磁碟時（實例：rustup 解壓上萬個小檔），verylow 讀取可能好幾分鐘都拿不到磁碟，進度看起來像卡住。這是背景優先權的正常行為。需要持續前進時改用 `--priority low`。
3. **SSD 閒置後的第一輪會變慢。** 沒有 DRAM 的 SSD 閒置一段時間後會做內部整理。這時主機讀取會被拖慢 10–20 秒，期間被讀的檔案全部都慢。立刻重讀也還在這段時間內，所以程式等整輪掃完再確認，第二次確認至少隔 30 秒。

## 風險

使用前請先備份重要資料。

- **陰影複製（還原點）**：搬移叢集會讓 Windows 陰影複製的差異區變大，舊的還原點可能被自動刪除。刷新前程式會顯示目前有幾份陰影複製。
- **寫入量**：刷新會把慢的檔案完整寫一次。SSD 的寫入壽命有限，不要頻繁地對整顆磁碟執行。
- **共用衝突**：Windows 上偵測其他寫入者時，程式會用「不允許別人寫入」的模式開檔一瞬間。如果剛好有程式在這一瞬間要開檔寫入，對方會收到共用衝突。VHDX 測試有量這個影響，見測試結果。
- **背景負載**：刷新期間磁碟會比較忙。預設用最低的 I/O 優先權，讓其他程式優先。
- **未驗證的平台**：Linux ext4 的刷新還沒在實機上驗證，只通過編譯檢查。

## 測試

### Windows：VHDX 安全驗證

測試在一個 4 GB 的 VHDX 上進行，不碰真正的磁碟。步驟 2–4 需要系統管理員 PowerShell。

1. 用一般權限建置：`cargo build --release`。
2. 建立並掛載 VHDX：`& test\vhdx-create.ps1`。
3. 執行完整測試：`& test\run-vhdx-test.ps1`。
4. 測完卸載：`& test\vhdx-detach.ps1`。

`run-vhdx-test.ps1` 會依序做這些事：

1. 產生約 1,100 個測試檔（0 B 到 200 MB，含刻意做出的碎片檔），記錄 SHA256。
2. 重新掛載 VHDX 清掉快取，掃描並比對雜湊。
3. 啟動修改程式，持續改寫約一成的檔案。修改方式有開關檔寫入、長期開著寫入、記憶體對應寫入、追加、截短。
4. 刷新全部檔案，在第 150 個檔案時中斷，再用 `--resume` 續跑，途中暫停 6 秒。
5. 停止修改程式，重新掛載後驗證：沒被修改的檔案雜湊必須完全一致，被修改的檔案必須是最新版本。
6. 檢查實體位置確實改變、搬移不會改動時間戳記，再執行 `chkdsk /scan`。

結果寫在 `output\vhdx-test-<時間>\summary.md`。

## 授權

尚未選定。開源前請加上 LICENSE 檔案。
