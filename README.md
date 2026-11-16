# Mattermost 指令傳訊（mmsend）

在 Windows 的 cmd、PowerShell、批次檔或工作排程器中，以一行指令傳送 Mattermost 訊息。

## 開始使用

1. 至 [Releases](../../releases) 下載 `mmsend.exe`。
2. 按兩下開啟，輸入 Mattermost 網址、帳號與密碼登入。
   - 首次開啟時若出現「Windows 已保護您的電腦」，請點選「其他資訊」→「仍要執行」。
3. 選擇人員或頻道。首次使用時請按「安裝」：程式會複製到 `%LOCALAPPDATA%\Programs\mmsend`、
   加入使用者 PATH，並在開始功能表建立「Mattermost 指令傳訊」。之後只需輸入 `mmsend` 即可執行，
   下載的檔案可以刪除。安裝後須**重新開啟** cmd 視窗才能使用 `mmsend`。
4. 輸入訊息後按「複製指令」，貼到 cmd 執行。人員清單最上方是您自己，可先傳送給自己測試。

## 指令

```bat
mmsend --user alice "報表跑完了"
mmsend --channel dev/town-square "部署完成"
mmsend --group alice,bob "群組訊息"

rem 多行或含雙引號的訊息：寫在文字檔中
mmsend --user alice --file msg.txt

rem 傳送其他指令的輸出；--code 以程式碼區塊顯示，保留欄位對齊
ipconfig | mmsend --user alice --code

rem 附加檔案（可重複指定，最多 10 個）
mmsend --channel dev/reports --attach 日報.xlsx "今日報表"
```

訊息超過 Mattermost 單則上限時，會改為附加「完整內容.txt」，訊息中只顯示前 20 行。
網路或伺服器暫時異常時會自動重試兩次，不會重複發送。
每次執行的結果（含失敗原因）可在設定視窗右上角「⋯」→「傳送紀錄」查看。

`--channel` 的格式為「團隊/頻道」，使用網址中的名稱（`https://chat.example.com/<團隊>/channels/<頻道>`）。
`--group` 列出除您以外的成員帳號，順序不限。

### 釘選與別名

在設定視窗選取對象後按右上角的圖釘，就會固定在清單最上方，並可取一個別名：

```bat
mmsend --to 值班 "伺服器異常"
```

換人值班時只要在設定視窗把別名改給新的對象，所有批次檔都不用修改。
別名存在這台電腦的這個 Windows 帳號中，批次檔拿到其他電腦需要另外設定。

完整說明、範例與供批次檔判斷的結束代碼，請執行 `mmsend -h`（或 `mmsend /?`）。

### 給腳本或 AI 使用

不開設定視窗也能從頭做到傳送：

```bat
mmsend status      :: 版本、是否已安裝、是否已登入（會連線確認）
mmsend login       :: 只開啟登入視窗，登入後自動關閉
mmsend find 王小明 :: 尋找人員、頻道或別名，每行開頭就是傳送時要用的參數
mmsend --user alice --dry-run "訊息"  :: 試跑：顯示收件對象的姓名與內容，不傳送
mmsend alias set 值班 --user alice    :: 設定別名（mmsend alias 列出，mmsend alias -h 看全部用法）
```

`mmsend login` 的視窗固定在最上層，登入後自動關閉並顯示登入的帳號；未登入就關閉視窗時，結束代碼為 3。
密碼只在視窗中輸入，呼叫的一方拿不到。

## 帳號密碼的存放位置

- 密碼與登入 token 存放於 **Windows 認證管理員**（控制台 → 認證管理員 → Windows 認證 → `mmsend`），
  由 Windows 以您的帳號加密，換成其他 Windows 帳號或其他電腦皆無法讀取。
- 網址與帳號存放於 `%APPDATA%\mmsend\config.json`。
- token 過期時，程式會自動以儲存的密碼重新登入。按「登出」會清除密碼與 token。

## 移除

從開始功能表開啟「Mattermost 指令傳訊」，按右上角「⋯」→「解除安裝…」。
程式、PATH 設定、開始功能表捷徑、登入資料與傳送紀錄會一併移除。

## 開發

```bat
cargo test
cargo run              :: 開啟設定視窗
cargo run -- --help    :: 指令說明
```

發版：修改 `Cargo.toml` 的 `version`，commit 後推送同版號的 tag（例如 `v0.2.0`），
GitHub Actions 會編譯並將 `mmsend.exe` 發佈至 Releases。

---

Mattermost is a trademark of Mattermost, Inc. 本專案是獨立的第三方工具，與 Mattermost, Inc. 無關，也未經其認可。
