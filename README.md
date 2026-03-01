# Voice Input

A macOS desktop app that lets you dictate text into any application using a global press-to-talk shortcut. All speech recognition runs locally — no cloud APIs, no data leaving your machine.

## Features

- **Global press-to-talk** — hold `Cmd+Shift+Space` to record, release to transcribe and type
- **100% local** — uses [Whisper](https://github.com/openai/whisper) (via whisper.cpp) with Metal acceleration on Apple Silicon
- **Optional LLM correction** — post-process transcriptions through a local [Ollama](https://ollama.com) model to fix proper nouns, punctuation, and language-specific quirks
- **Streaming mode** — transcribes in chunks while you speak, useful for long dictations
- **Transcription history** — browse and copy past transcriptions from the app window
- **Configurable** — shortcut, language, model path, and LLM settings are all adjustable

## How It Works

```
Hold shortcut → Record audio (cpal)
             → Release shortcut
             → Whisper transcription (whisper-rs, Metal)
             → [Optional] LLM correction (Ollama REST API)
             → Copy to clipboard + simulate paste (Cmd+V) into active app
             → Save to history
```

## Tech Stack

| Layer | Technology |
|---|---|
| Frontend | Svelte 5 + TypeScript + Vite |
| Backend | Rust + Tauri 2 |
| Speech-to-Text | whisper-rs (whisper.cpp bindings, Metal GPU acceleration) |
| LLM Correction | Ollama REST API |
| Audio capture | cpal |
| Clipboard + paste | arboard + enigo |

## Prerequisites

- **macOS** (Apple Silicon recommended for Metal acceleration; Intel Macs will fall back to CPU)
- **Rust toolchain** — install via [rustup](https://rustup.rs)
- **Node.js 18+**
- **Whisper model file** — `ggml-medium.bin` placed at:
  ```
  ~/Library/Application Support/com.voice-input.app/models/ggml-medium.bin
  ```
  Download from [huggingface.co/ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp)
- **Ollama** (optional) — required only if LLM correction is enabled. Install from [ollama.com](https://ollama.com), then pull your model:
  ```bash
  ollama pull gemma3:4b
  ```

## Build & Run

All commands run from the `app/` directory.

**Install dependencies:**
```bash
npm install
```

**Development (with hot reload):**
```bash
npm run tauri dev
```

**Production build:**
```bash
npm run tauri build
```

The built `.app` bundle will be in `app/src-tauri/target/release/bundle/macos/`.

## Configuration

Config file: `~/Library/Application Support/com.voice-input.app/config.json`

The file is created automatically on first run with defaults. Edit it while the app is closed, or use the Settings panel inside the app.

| Key | Default | Description |
|---|---|---|
| `shortcut` | `"CmdOrCtrl+Shift+Space"` | Global press-to-talk shortcut |
| `quit_shortcut` | `"CmdOrCtrl+Alt+Q"` | Quit the app |
| `language` | `"zh"` | Whisper language code (`"en"`, `"zh"`, `"ja"`, etc.) |
| `model_path` | `…/models/ggml-medium.bin` | Absolute path to the Whisper model file |
| `llm_enabled` | `false` | Enable Ollama LLM post-processing |
| `llm_model` | `"gemma3:4b"` | Ollama model name |
| `llm_endpoint` | `"http://localhost:11434"` | Ollama API base URL |
| `sound_enabled` | `true` | Play sound effects on record start/stop |
| `max_history` | `50` | Maximum number of history entries to keep |

## Project Structure

```
app/
├── src/                  # Svelte frontend
└── src-tauri/
    └── src/
        ├── audio.rs      # Audio recording (cpal)
        ├── whisper.rs    # Whisper model loading and transcription
        ├── llm.rs        # Ollama LLM correction
        ├── commands.rs   # Tauri IPC command handlers
        ├── config.rs     # App configuration (JSON file)
        ├── paste.rs      # Clipboard paste into active app
        ├── history.rs    # Transcription history
        └── model.rs      # Model file management
```

## Known Limitations

- **macOS only** — the paste mechanism uses macOS-specific keyboard simulation; Linux and Windows are not supported
- **No model auto-download** — the Whisper model file must be placed manually; in-app download is not yet implemented
- **File transcription not yet implemented** — only live microphone input is supported; transcribing audio files is planned for a future release
