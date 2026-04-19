# AIpuss-browser

AI-first 瀏覽器自動化 CLI，基於 Rust + CDP。

這是 [opencli](https://github.com/jackwener/opencli) 的分支，專為 Hermes Agent 整合優化。

## 核心功能

- **Daemon 模式** — 啟動一次，CLI 指令或 WebSocket 持續控制
- **無頭 Chromium** — 自帶 Chrome，不需要系統已安裝
- **無障礙樹快照** — `@e1`、`@e2` 引用，精確定位元素
- **語義定位器** — 按 role、text、label、placeholder、ARIA 查找
- **瀏覽器設定檔** — 復用已登入的 Chrome 狀態
- **Session 隔離** — 多個獨立瀏覽器實例
- **網路攔截** — Mock、封鎖、錄製 HTTP 請求
- **狀態持久化** — 自動儲存/還原 cookies 和 localStorage
- **Auth Vault** — 加密儲存憑證
- **截圖差異比對** — 回歸測試用

## 安裝

```bash
git clone https://github.com/whypuss/AIpuss-browser.git
cd AIpuss-browser
pnpm install
pnpm build          # 建 Next.js dashboard
pnpm build:native   # 建 Rust binary（需先裝 Rust: rustup.rs）
pnpm link --global  # 連結為全域指令 aipuss-browser
```

## 快速開始

```bash
# 開啟頁面
aipuss-browser open example.com

# 取得無障礙樹（互動元素引用）
aipuss-browser snapshot -i

# 按引用點擊 / 填寫
aipuss-browser click @e2
aipuss-browser fill @e3 "test@example.com"
aipuss-browser press Enter

# 截圖
aipuss-browser screenshot

# 關閉
aipuss-browser close
```

## Daemon 模式（推薦給 AI Agent 用）

啟動一次，後續所有指令復用同一個瀏覽器實例。

```bash
# 啟動 daemon
aipuss-browser stream enable
# 輸出：Stream enabled on port 62097

#之後所有指令自動連到 daemon
aipuss-browser open github.com
aipuss-browser snapshot -i
aipuss-browser click @e5
aipuss-browser close

# 停止 daemon
aipuss-browser stream disable
```

## Session 管理

### 自動儲存登入狀態（推薦）

```bash
# 第一次：開登入頁，登入後關閉，狀態自動保存
aipuss-browser --session-name myapp open https://app.example.com/login
# 手動完成登入
aipuss-browser close

# 之後直接還原登入狀態
aipuss-browser --session-name myapp open https://app.example.com/dashboard
```

### 復用已登入的 Chrome

```bash
aipuss-browser --profile Default open github.com
```

### Auth Vault

```bash
# 保存密碼
echo "$PASSWORD" | aipuss-browser auth save myapp \
  --url https://app.example.com/login \
  --username me@example.com \
  --password-stdin

# 之後自動登入
aipuss-browser auth login myapp
```

## 常用指令

```bash
# 導航
aipuss-browser open <url>
aipuss-browser close

# 快照
aipuss-browser snapshot -i              # 互動元素引用
aipuss-browser snapshot -i --urls       # 含連結 URL

# 操作
aipuss-browser click @e1
aipuss-browser fill @e2 "文字"
aipuss-browser type @e2 "打字不放"
aipuss-browser select @e1 "選項"
aipuss-browser scroll down 500

# 等待
aipuss-browser wait 2000               # 等毫秒
aipuss-browser wait @e1               # 等元素出現
aipuss-browser wait --url "**/done"   # 等 URL 符合

# 截圖
aipuss-browser screenshot
aipuss-browser screenshot --full
aipuss-browser screenshot --annotate    # 標元素編號

# JS 執行
aipuss-browser eval 'document.title'
```

## Skyvern Adapter（LLM 驅動的瀏覽器 Agent）

`skyvern_adapter.rs` 實現了 Skyvern 相容的 REST API surface，讓 Skyvern 生態的 agent（SDK）可以直接對接 AIpuss 的 Rust/CDP 底層。

### 核心概念：Protocol Bridge

```
Skyvern = Python (Playwright) + LLM loop → 協議
AIpuss = Rust (CDP)             → 原生底層
              ↑____________ Skyvern Adapter (橋接)
```

Adapter 把 Skyvern 的 LLM 驅動 workflow（observe → think → act → verify → repeat）翻譯成 AIpuss 的 CDP 命令，無需更換 agent 端代碼。

### 支援的 SDK Action

| SDK Action | 說明 |
|------------|------|
| `ai_click` | 按引用或意圖點擊元素 |
| `ai_input_text` | 填入文字 |
| `ai_select_option` | 選擇下拉選項 |
| `ai_upload_file` | 上傳文件 |
| `ai_act` | 自然語言動作（需完整 task loop） |
| `extract` | 結構化資料提取 |
| `validate` | 頁面驗證 |
| `prompt` | LLM 提示並取回回應 |
| `locate_element` | 元素定位 |

### LLM Provider

支援 OpenAI（GPT-4o）和 Anthropic（Claude）：

```rust
// OpenAI
let provider = OpenAILLMProvider::new(api_key, "gpt-4o".to_string());

// Anthropic
let provider = AnthropicLLMProvider::new(api_key, "claude-3-5-sonnet".to_string());
```

### REST API 端點

| 端點 | 方法 | 說明 |
|------|------|------|
| `/skyvern/tasks` | POST | 建立並執行 task |
| `/skyvern/tasks/{id}` | GET | 查詢 task 狀態 |
| `/skyvern/run_action` | POST | 執行單一 SDK action |

### Task 建立與執行

```json
// POST /skyvern/tasks
{
  "url": "https://github.com/search?q=rust+web+framework",
  "navigation_goal": "搜尋 Rust Web 框架，找到 stars 最多的前三個 repo",
  "complete_criterion": "頁面上顯示了至少 3 個 repo 條目，且每個都有 star 數量",
  "max_steps": 20
}
```

### Task 狀態

`created` → `pending` → `running` → `completed` / `failed` / `max_steps_reached`

每個 step 記錄 action_history（action_type、reasoning、confidence、result）。

### 與現有模組的整合

- `AgentSnapshotOutput`（`agent_snapshot.rs`）→ LLM 的 accessible elements
- `AgentStateTracker`（`agent_state.rs`）→ action_history 和 StateDiff
- `PrefetchCache`（`prefetch.rs`）→ background 預取
- CDP 命令（`actions.rs`）→ act 層的實際執行

## 引用生命周期

`@e1`、`@e2` 等引用在**頁面變化後失效**（點擊連結、表單送出、動態載入）。

每次頁面變化後必須重新 snapshot。

## Hermes Agent 整合

Daemon 啟動後，Hermes 的 `browser_tool.py` 自動透過 CLI 或 WebSocket 控制瀏覽器，支援導航、快照、截圖、點擊、填寫、滾動、資料抓取。

### macOS 24/7 後台運行

```bash
# 安裝 watchdog + launchd service
launchctl load ~/Library/LaunchAgents/com.hermes.aipuss-watchdog.plist

# 查看狀態
tail -f /tmp/aipuss-watchdog.log
```

## License

MIT
