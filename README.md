<p align="center">
  <img src="docs/logo.png" width="96" alt="">
</p>

<h1 align="center">Mattermost 指令傳訊（mmsend）</h1>

<p align="center">
  讓 AI 用你的 Mattermost 帳號傳訊息 —— 也是批次檔、排程裡的一行指令。
</p>

<p align="center">
  <a href="../../releases/latest"><img src="https://img.shields.io/github/v/release/pp771007/mmsend?label=%E6%9C%80%E6%96%B0%E7%89%88" alt="最新版"></a>
  <a href="../../releases"><img src="https://img.shields.io/github/downloads/pp771007/mmsend/total?label=%E4%B8%8B%E8%BC%89" alt="下載次數"></a>
  <img src="https://img.shields.io/badge/Windows-10%20%7C%2011-0078D6?logo=windows" alt="Windows 10 | 11">
  <a href="skills/mmsend/SKILL.md"><img src="https://img.shields.io/badge/Claude%20Code-skill-D97757?logo=claude&logoColor=white" alt="Claude Code skill"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/pp771007/mmsend" alt="MIT"></a>
</p>

```text
你  ▸ 跟王小明說報表跑完了

AI  ▸ mmsend find 王小明
      人員：
        --user alice  王小明

    ▸ mmsend --user alice --dry-run "報表跑完了"
      試跑，未傳送
      收件對象：@alice 王小明
      內容：
        報表跑完了

      要傳給 @alice 王小明 嗎？

你  ▸ 好

AI  ▸ mmsend --user alice "報表跑完了"
      已傳送給 @alice 王小明（訊息 ID：ykt8phtqe78f3p9erz43raxauh）
```

## 為什麼適合交給 AI

- 🧰 **從零開始**：還沒安裝時 AI 會自己下載、核對檢查碼、安裝，你不必打開任何東西。
- 🔑 **密碼不經過 AI**：`mmsend login` 在你的螢幕上跳出登入視窗，AI 只拿得到「登入成功」。
- 👀 **先試跑、再傳送**：`--dry-run` 列出收件人姓名與訊息全文，你說好才送出。
- 🔎 **找人不用猜**：`mmsend find 王小明` 每一行開頭就是傳送要用的參數。
- ↩️ **傳錯收得回**：`mmsend delete --last` 刪掉剛傳的那則。
- 🧾 **好判讀**：成功印一行在 stdout，失敗印在 stderr 並寫下一步該做什麼；結束代碼分類清楚。

## 開始使用

### 交給 AI（Claude Code）

1. 把 [`skills/mmsend`](skills/mmsend) 資料夾複製到 `%USERPROFILE%\.claude\skills\`。
2. 跟它說「傳 Mattermost 跟王小明說報表好了」。

第一次它會自己安裝 mmsend，並請你在跳出的視窗登入。其他 AI 工具把 [`SKILL.md`](skills/mmsend/SKILL.md) 的內容當成說明給它即可。

### 自己用

1. 至 [Releases](../../releases/latest) 下載 `mmsend.exe`，按兩下開啟就會自動安裝（出現「Windows 已保護您的電腦」時，點「其他資訊」→「仍要執行」）。
2. 登入：伺服器網址、帳號、密碼；有兩步驟驗證會接著要驗證碼。公司用 SSO、或不想存密碼時，改選「存取權杖」（Mattermost「個人設定 › 安全性」產生）。
3. 選人員或頻道、輸入訊息，按「複製指令」貼進批次檔；單行訊息也可以直接「立即傳送」。

安裝後**新開的** cmd 視窗才認得 `mmsend`。下載的檔案可以刪除；留著的話，按兩下也會開啟已安裝的最新版。

## 指令

```bat
mmsend --user alice "報表跑完了"
mmsend --channel dev/town-square "部署完成"
mmsend --group alice,bob "會議改到三點"
mmsend --to 值班 "伺服器異常"

rem 訊息裡的 " 寫成 ""、% 寫成 %%（設定視窗複製的指令已處理好）
mmsend --user alice "進度 100%% ""完成"""

rem 多行訊息：寫在文字檔中
mmsend --user alice --file msg.txt

rem 傳送其他指令的輸出；--code 以程式碼區塊顯示，保留欄位對齊
ipconfig | mmsend --user alice --code

rem 附加檔案（可重複指定，最多 10 個）
mmsend --channel dev/reports --attach 日報.xlsx "今日報表"
```

```bat
mmsend status                       :: 版本、安裝、有沒有新版、登入狀態
mmsend install                      :: 安裝這個檔案，並顯示安裝後的完整路徑
mmsend update                       :: 更新至最新版本
mmsend login                        :: 只開登入視窗，登入後自動關閉
mmsend find 王小明                  :: 找人員、頻道或別名
mmsend --user alice --dry-run "訊息" :: 試跑：顯示收件人姓名與內容，不傳送
mmsend alias set 值班 --user alice   :: 設定別名（mmsend alias 列出）
mmsend delete --last                :: 刪除最近一次傳送的訊息（或 mmsend delete <訊息 ID>）
```

- `--channel` 用網址裡的名稱：`https://chat.example.com/<團隊>/channels/<頻道>`。
- `--group` 列出除了自己以外的成員，順序不限。
- 訊息照 Mattermost 的 Markdown 顯示；超過單則上限時改附「完整內容.txt」，訊息只顯示前 20 行。
- 網路或伺服器暫時異常會自動重試兩次，不會重複發送。
- 每個指令的完整說明與給批次檔判斷的結束代碼：`mmsend -h`、`mmsend <指令> -h`。

### 別名：換人不用改批次檔

`mmsend --to 值班` 傳給別名指的對象。換人值班時把別名改給新的人（`mmsend alias set 值班 --user bob`，或在設定視窗的「別名」欄），所有批次檔都不用動。
一個對象只能有一個別名；別名存在這台電腦的這個 Windows 帳號裡。

<details>
<summary><b>設定視窗</b></summary>

不帶參數開啟 `mmsend`（或開始功能表的「Mattermost 指令傳訊」）：

- 左邊是釘選、最近私訊與頻道，最上方是您自己，可先傳給自己測試；其他人用上方的搜尋。
- 選了對象後可以「釘選」固定在最上方，或在「別名」欄取名字（也會一併釘選）。
- 右上角「⋯」：傳送紀錄（每次執行的結果與失敗原因）、檢查更新、登出、解除安裝。

</details>

<details>
<summary><b>帳號密碼存放在哪裡</b></summary>

- 密碼與登入 token 在 **Windows 認證管理員**（控制台 → 認證管理員 → Windows 認證 → `mmsend`），由 Windows 以您的帳號加密，換成其他 Windows 帳號或其他電腦都讀不到。
- 伺服器網址與帳號在 `%APPDATA%\mmsend\config.json`。
- token 過期時會自動用儲存的密碼重新登入；「⋯」→「登出…」會清除密碼與 token。
- 用存取權杖或兩步驟驗證登入時不保存密碼；登入過期（或權杖被撤銷）時需要重新登入。

</details>

<details>
<summary><b>更新與移除</b></summary>

- 設定視窗每天開啟時檢查一次新版，有新版會在上方顯示「更新」；也可以「⋯」→「檢查更新」，或執行 `mmsend update`。批次檔、排程傳送時不會檢查。
- 移除：設定視窗「⋯」→「解除安裝…」，程式、PATH、開始功能表捷徑、登入資料與傳送紀錄一併移除。

</details>

<details>
<summary><b>開發</b></summary>

```bat
cargo test
cargo run              :: 開啟設定視窗
cargo run -- --help    :: 指令說明
```

開發版（`cargo run` / `cargo build`）是獨立的「Mattermost 指令傳訊（開發版）」：設定、登入、傳送紀錄存在 `%APPDATA%\mmsend-dev`，安裝為 `mmsend-dev` 指令，不會動到正式版的資料。

測試用的 Mattermost（需要 Docker）：

```bat
python dev/test-server.py up      :: 啟動並讓開發版登入測試帳號 tester
python dev/test-server.py down    :: 停止
```

內含 alice 等幾位同事、公開與私人頻道、一個 tester 沒加入的頻道，以及用存取權杖、兩步驟驗證登入的帳號。
其他指令（取得驗證碼、重建）與帳號說明：不帶參數執行 `python dev/test-server.py`。

`docs/logo.png` 由 `src/icon.rs` 產生：改了圖示後執行 `cargo run --example export_logo -- docs/logo.png`。

發版：修改 `Cargo.toml` 的 `version`，commit 後推送同版號的 tag（例如 `v0.2.0`），GitHub Actions 會編譯並將 `mmsend.exe` 發佈至 Releases。

</details>

---

<sub>Mattermost is a trademark of Mattermost, Inc. 本專案是獨立的第三方工具，與 Mattermost, Inc. 無關，也未經其認可。</sub>
