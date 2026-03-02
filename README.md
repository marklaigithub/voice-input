# Voice Input

macOS 桌面應用程式，透過全域「按住說話」快捷鍵將語音轉為文字，直接輸入到任何應用程式中。語音辨識完全在本機執行——不需要雲端 API，資料不會離開你的電腦。

## 功能特色

- **全域按住說話** — 按住 `Cmd+Shift+Space` 錄音，放開後自動轉錄並輸入文字
- **100% 本機運行** — 使用 [Whisper](https://github.com/openai/whisper)（透過 whisper.cpp），在 Apple Silicon 上支援 Metal GPU 加速
- **可選 LLM 校正** — 透過本機 [Ollama](https://ollama.com) 模型後處理轉錄結果，修正專有名詞、標點符號和語言特有問題
- **串流模式** — 說話時即時分段轉錄，適合長篇口述
- **浮動錄音指示器** — 錄音時顯示半透明浮動視窗，含即時波形動畫和計時器，可拖曳定位
- **應用程式內模型下載** — 首次使用可直接在應用程式內下載 Whisper 模型，支援斷點續傳和 SHA256 驗證
- **音訊檔案轉錄** — 支援轉錄 WAV 音訊檔案（16-bit、24-bit、32-bit float）
- **轉錄歷史** — 在應用程式視窗中瀏覽和複製過去的轉錄記錄
- **可自訂設定** — 快捷鍵、語言、模型路徑和 LLM 設定皆可調整

## 運作原理

```
按住快捷鍵 → 錄音（cpal）
           → 放開快捷鍵
           → Whisper 語音轉錄（whisper-rs, Metal）
           → [可選] LLM 校正（Ollama REST API）
           → 複製到剪貼簿 + 模擬貼上（Cmd+V）到前台應用程式
           → 儲存到歷史記錄
```

## 技術架構

| 層級 | 技術 |
|---|---|
| 前端 | Svelte 5 + TypeScript + Vite |
| 後端 | Rust + Tauri 2 |
| 語音轉文字 | whisper-rs（whisper.cpp 綁定，Metal GPU 加速）|
| LLM 校正 | Ollama REST API |
| 音訊擷取 | cpal |
| 剪貼簿 + 貼上 | arboard + enigo |

## 系統需求

- **macOS**（建議 Apple Silicon 以獲得 Metal 加速；Intel Mac 會退回 CPU 運算）
- **Rust 工具鏈** — 透過 [rustup](https://rustup.rs) 安裝
- **Node.js 18+**
- **Whisper 模型檔** — 首次啟動時可在應用程式內直接下載，或手動將 `ggml-medium.bin` 放在：
  ```
  ~/Library/Application Support/com.voice-input.app/models/ggml-medium.bin
  ```
  手動下載來源：[huggingface.co/ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp)
- **Ollama**（可選）— 僅在啟用 LLM 校正時需要。從 [ollama.com](https://ollama.com) 安裝後，拉取模型：
  ```bash
  ollama pull gemma3:4b
  ```

## macOS 權限設定

Voice Input 需要兩項系統權限才能正常運作：

1. **麥克風** — 首次錄音時 macOS 會自動彈出授權提示
2. **輔助使用** — 用於模擬 `Cmd+V` 貼上到前台應用程式

授予輔助使用權限：

> 系統設定 → 隱私權與安全性 → 輔助使用 → 加入 Voice Input（或執行 `tauri dev` 的終端機）

**重要：** macOS 將輔助使用權限綁定到應用程式二進位檔的 hash。每次重新建置應用程式後，舊的權限會失效，必須重新授予。如果轉錄正常但文字沒有出現，幾乎可以確定是這個原因。

如果缺少輔助使用權限，Voice Input 會退回到僅複製到剪貼簿——你會看到通知提示手動 `Cmd+V` 貼上。

## 建置與執行

所有指令在 `app/` 目錄下執行。

**安裝相依套件：**
```bash
npm install
```

**開發模式（支援熱重載）：**
```bash
npm run tauri dev
```

**正式建置：**
```bash
npm run tauri build
```

建置好的 `.app` 套件會在 `app/src-tauri/target/release/bundle/macos/`。

**執行測試：**
```bash
cargo test --manifest-path app/src-tauri/Cargo.toml
```

## 設定說明

設定檔位置：`~/Library/Application Support/com.voice-input.app/config.json`

首次執行時會自動建立預設設定檔。可在應用程式關閉時手動編輯，或使用應用程式內的設定面板。

| 設定項 | 預設值 | 說明 |
|---|---|---|
| `shortcut` | `"CmdOrCtrl+Shift+Space"` | 全域按住說話快捷鍵 |
| `quit_shortcut` | `"CmdOrCtrl+Alt+Q"` | 退出應用程式 |
| `language` | `"zh"` | Whisper 語言代碼（`"en"`、`"zh"`、`"ja"` 等）|
| `model_path` | `…/models/ggml-medium.bin` | Whisper 模型檔的絕對路徑 |
| `llm_enabled` | `false` | 啟用 Ollama LLM 後處理 |
| `llm_model` | `"gemma3:4b"` | Ollama 模型名稱 |
| `llm_endpoint` | `"http://localhost:11434"` | Ollama API 基礎 URL |
| `sound_enabled` | `true` | 錄音開始/結束時播放音效 |
| `max_history` | `50` | 歷史記錄保留的最大筆數 |

## 專案結構

```
app/
├── src/                  # Svelte 前端
└── src-tauri/
    └── src/
        ├── lib.rs        # Tauri 應用程式設定、全域快捷鍵、系統匣選單
        ├── audio.rs      # 音訊錄製 + 重取樣（cpal）
        ├── whisper.rs    # Whisper 模型載入與轉錄
        ├── llm.rs        # Ollama LLM 校正 + 幻覺守衛
        ├── commands.rs   # Tauri IPC 指令處理器
        ├── config.rs     # 應用程式設定（JSON 檔案）
        ├── paste.rs      # 剪貼簿貼上到前台應用程式
        ├── history.rs    # 轉錄歷史記錄
        └── model.rs      # 模型檔案管理
```

## 已知限制

- **僅支援 macOS** — 貼上機制使用 macOS 專屬的鍵盤模擬；不支援 Linux 和 Windows
