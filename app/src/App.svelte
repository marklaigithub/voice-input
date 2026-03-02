<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import { listen, emit } from '@tauri-apps/api/event'
  import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
  import { onMount } from 'svelte'
  import { appState, modelLoaded, config, history, lastTranscription, errorMessage } from './lib/store'
  import type { AppStatus, HistoryEntry } from './lib/types'
  import AudioWaveform from './lib/AudioWaveform.svelte'

  let activeTab = $state<'status' | 'history' | 'settings'>('status')

  onMount(async () => {
    try {
      // Load initial state
      const status: AppStatus = await invoke('get_app_state')
      $modelLoaded = status.model_loaded
      $config = status.config
      $appState = status.model_loaded ? 'idle' : 'loading'

      // Load history
      const h: HistoryEntry[] = await invoke('get_history')
      $history = h

      // If model not loaded, try to init after a delay
      // (let Tauri fully initialize + Metal shaders compile first)
      if (!status.model_loaded) {
        const hasModel: boolean = await invoke('check_model')
        if (hasModel) {
          $appState = 'loading'
          // Delay model loading to avoid memory pressure during startup
          await new Promise(r => setTimeout(r, 2000))
          try {
            await invoke('init_whisper')
            $modelLoaded = true
            $appState = 'idle'
          } catch (loadErr) {
            $errorMessage = `Model 載入失敗：${loadErr}`
            $appState = 'idle'
          }
        }
      }
    } catch (e) {
      $errorMessage = String(e)
      $appState = 'idle'
    }

    // Check LLM availability
    checkLlmStatus()

    // Listen for shortcut events (press-to-talk)
    await listen<string>('shortcut-event', async (event) => {
      if (event.payload === 'pressed' && $appState === 'idle' && $modelLoaded) {
        try {
          $appState = 'recording'
          llmApplied = null
          await invoke('start_recording')
          // Start waveform animation + streaming transcription
          startAudioLevelListener()
          startStreaming()
          // Notify indicator window
          await emit('recording-started')
          showIndicatorWindow()
        } catch (e) {
          $errorMessage = String(e)
          $appState = 'idle'
        }
      } else if (event.payload === 'released' && $appState === 'recording') {
        stopAudioLevelListener()
        await stopStreaming()
        // Notify indicator window and hide it
        await emit('recording-stopped')
        hideIndicatorWindow()
        try {
          $appState = 'transcribing'
          const text: string = await invoke('stop_recording_and_transcribe')
          if (text) {
            $lastTranscription = text
            // Refresh history
            const h: HistoryEntry[] = await invoke('get_history')
            $history = h
          }
          $appState = 'idle'
        } catch (e) {
          const err = String(e)
          if (err === 'too_short') {
            // Silently ignore short presses
          } else {
            $errorMessage = err
          }
          $appState = 'idle'
        }
      }
    })

    // Listen for transcription complete events
    await listen<string>('transcription-complete', (event) => {
      $lastTranscription = event.payload
    })

    // Listen for LLM correction events
    await listen('llm-correction-start', () => {
      if ($appState === 'transcribing') {
        $appState = 'correcting'
      }
    })

    await listen<boolean>('llm-correction-done', (event) => {
      llmApplied = event.payload
    })

    await listen<string>('paste-fallback', (event) => {
      $errorMessage = `無法自動貼上（${event.payload}），文字已複製到剪貼簿，請手動 ⌘V`
    })
  })

  async function handleClearHistory() {
    await invoke('clear_history')
    $history = []
  }

  function stateLabel(state: string): string {
    switch (state) {
      case 'idle': return 'Ready'
      case 'recording': return 'Recording...'
      case 'transcribing': return 'Transcribing...'
      case 'correcting': return 'Correcting...'
      case 'loading': return 'Loading model...'
      default: return state
    }
  }

  function stateColor(state: string): string {
    switch (state) {
      case 'recording': return '#ef4444'
      case 'transcribing': return '#f59e0b'
      case 'correcting': return '#8b5cf6'
      case 'loading': return '#6b7280'
      default: return '#22c55e'
    }
  }

  function formatTimestamp(ts: string): string {
    try {
      const d = new Date(ts)
      return d.toLocaleTimeString('zh-TW', { hour: '2-digit', minute: '2-digit' })
    } catch {
      return ts
    }
  }

  function formatSource(source: HistoryEntry['source']): string {
    if (typeof source === 'string') {
      return source === 'PressToTalk' ? 'PTT' : source
    }
    if (source && typeof source === 'object') {
      if ('PressToTalk' in source) return 'PTT'
      if ('File' in source) return (source as { File: string }).File
    }
    return '?'
  }

  function formatShortcut(raw: string): string {
    return raw
      .replace('CmdOrCtrl', '⌘')
      .replace('Cmd', '⌘')
      .replace('Ctrl', '⌃')
      .replace('Alt', '⌥')
      .replace('Shift', '⇧')
      .replace('Space', 'Space')
      .replace(/\+/g, ' ')
  }

  // Audio level from backend event (emitted every ~80ms during recording)
  let audioLevel = $state(0)
  let unlistenAudioLevel: (() => void) | null = null

  async function startAudioLevelListener() {
    audioLevel = 0
    unlistenAudioLevel = await listen<number>('audio-level', (event) => {
      audioLevel = event.payload
    })
  }

  function stopAudioLevelListener() {
    if (unlistenAudioLevel) {
      unlistenAudioLevel()
      unlistenAudioLevel = null
    }
    audioLevel = 0
  }

  // Indicator window show/hide
  async function showIndicatorWindow() {
    if (!$config?.show_recording_indicator) return
    const win = WebviewWindow.getByLabel('indicator')
    if (!win) return
    // Restore saved position
    if ($config?.indicator_x != null && $config?.indicator_y != null) {
      try {
        const { LogicalPosition } = await import('@tauri-apps/api/dpi')
        await win.setPosition(new LogicalPosition($config.indicator_x, $config.indicator_y))
      } catch { /* ignore — will use default position */ }
    }
    await win.show()
  }

  async function hideIndicatorWindow() {
    const win = WebviewWindow.getByLabel('indicator')
    if (!win) return
    // Save current position before hiding
    try {
      const pos = await win.outerPosition()
      saveConfig({ indicator_x: pos.x, indicator_y: pos.y })
    } catch { /* ignore */ }
    await win.hide()
  }

  // Settings editing
  // Streaming transcription (preview only — no paste during recording)
  let streamingInterval: ReturnType<typeof setInterval> | null = $state(null)
  let streamingChunks = $state(0)
  let pendingChunk: Promise<void> | null = $state(null)
  const CHUNK_INTERVAL_MS = 6000 // transcribe every 6 seconds

  function startStreaming() {
    streamingChunks = 0
    streamingInterval = setInterval(() => {
      // Skip if previous chunk is still being transcribed (re-entry guard)
      if (pendingChunk) return

      const promise = (async () => {
        try {
          const result: string | null = await invoke('transcribe_chunk')
          if (result) {
            streamingChunks++
            $lastTranscription = result
          }
        } catch (e) {
          console.warn('Chunk transcription failed:', e)
        }
      })()
      pendingChunk = promise
      promise.then(() => { if (pendingChunk === promise) pendingChunk = null })
    }, CHUNK_INTERVAL_MS)
  }

  async function stopStreaming() {
    if (streamingInterval) {
      clearInterval(streamingInterval)
      streamingInterval = null
    }
    // Wait for any in-flight chunk to finish before stop_recording_and_transcribe
    if (pendingChunk) {
      await pendingChunk
      pendingChunk = null
    }
  }

  let editingLanguage = $state(false)
  let languageInput = $state('')
  let savingConfig = $state(false)
  let llmStatus = $state<{ available: boolean; model_available: boolean } | null>(null)
  let llmApplied = $state<boolean | null>(null)
  let editingLlmModel = $state(false)
  let llmModelInput = $state('')

  async function saveConfig(updates: Partial<import('./lib/types').AppConfig>) {
    if (!$config || savingConfig) return
    savingConfig = true
    try {
      const newConfig = { ...$config, ...updates }
      await invoke('save_config', { config: newConfig })
      $config = newConfig
    } catch (e) {
      $errorMessage = `設定儲存失敗：${e}`
    } finally {
      savingConfig = false
    }
  }

  function toggleSound() {
    if ($config) saveConfig({ sound_enabled: !$config.sound_enabled })
  }

  function toggleIndicator() {
    if ($config) saveConfig({ show_recording_indicator: !$config.show_recording_indicator })
  }

  function startEditLanguage() {
    languageInput = $config?.language ?? 'auto'
    editingLanguage = true
  }

  function saveLanguage() {
    saveConfig({ language: languageInput })
    editingLanguage = false
  }

  async function checkLlmStatus() {
    try {
      const status = await invoke<{ available: boolean; model_available: boolean; enabled: boolean; model: string }>('check_llm_status')
      llmStatus = { available: status.available, model_available: status.model_available }
    } catch {
      llmStatus = null
    }
  }

  // Model download
  let isDownloading = $state(false)
  let downloadProgress = $state(0)
  let downloadError = $state('')

  async function startModelDownload() {
    isDownloading = true
    downloadProgress = 0
    downloadError = ''

    const unlisten = await listen<{ downloaded: number; total: number; percentage: number }>('model-download-progress', (event) => {
      downloadProgress = Math.round(event.payload.percentage)
    })

    try {
      await invoke('download_model')
      // Download complete — load the model
      $appState = 'loading'
      await invoke('init_whisper')
      $modelLoaded = true
      $appState = 'idle'
    } catch (e) {
      downloadError = String(e)
    } finally {
      isDownloading = false
      unlisten()
    }
  }

  async function toggleLlm() {
    if ($config) {
      await saveConfig({ llm_enabled: !$config.llm_enabled })
      checkLlmStatus()
    }
  }

  function startEditLlmModel() {
    llmModelInput = $config?.llm_model ?? 'gemma3:4b'
    editingLlmModel = true
  }

  function saveLlmModel() {
    saveConfig({ llm_model: llmModelInput })
    editingLlmModel = false
  }
</script>

<main>
  <header>
    <div class="status-dot" style="background-color: {stateColor($appState)}"></div>
    <span class="status-text">{stateLabel($appState)}</span>
    {#if !$modelLoaded && !isDownloading}
      <span class="warning">Model not loaded</span>
      <button class="download-btn" onclick={startModelDownload}>Download</button>
    {/if}
    {#if isDownloading}
      <span class="download-info">Downloading... {downloadProgress}%</span>
    {/if}
  </header>

  {#if $errorMessage}
    <div class="error-bar">
      <span>{$errorMessage}</span>
      <button onclick={() => $errorMessage = ''}>×</button>
    </div>
  {/if}

  <nav>
    <button class:active={activeTab === 'status'} onclick={() => activeTab = 'status'}>Status</button>
    <button class:active={activeTab === 'history'} onclick={() => activeTab = 'history'}>History</button>
    <button class:active={activeTab === 'settings'} onclick={() => activeTab = 'settings'}>Settings</button>
  </nav>

  <section class="content">
    {#if activeTab === 'status'}
      <div class="status-panel">
        <div class="big-status">
          {#if $appState === 'recording'}
            <AudioWaveform level={audioLevel} />
          {:else}
            <div class="big-dot" style="background-color: {stateColor($appState)}"></div>
          {/if}
          <h2>{stateLabel($appState)}</h2>
        </div>
        {#if $lastTranscription}
          <div class="last-result">
            <span class="label-text">Last transcription:{#if llmApplied === true} <span class="llm-badge">LLM</span>{/if}</span>
            <p>{$lastTranscription}</p>
          </div>
        {/if}
        <div class="shortcut-hint">
          <p>Press <kbd>{formatShortcut($config?.shortcut ?? 'Cmd+Shift+Space')}</kbd> to talk</p>
        </div>
      </div>

    {:else if activeTab === 'history'}
      <div class="history-panel">
        <div class="history-header">
          <h3>History ({$history.length})</h3>
          {#if $history.length > 0}
            <button class="clear-btn" onclick={handleClearHistory}>Clear</button>
          {/if}
        </div>
        {#if $history.length === 0}
          <p class="empty">No transcriptions yet</p>
        {:else}
          <ul class="history-list">
            {#each [...$history].reverse() as entry}
              <li>
                <div class="entry-meta">
                  <span class="time">{formatTimestamp(entry.timestamp)}</span>
                  <span class="source">{formatSource(entry.source)}</span>
                  <span class="duration">{entry.duration_secs.toFixed(1)}s</span>
                </div>
                <p class="entry-text">{entry.text}</p>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

    {:else if activeTab === 'settings'}
      <div class="settings-panel">
        <div class="setting-item">
          <span class="label-text">Talk shortcut</span>
          <kbd>{formatShortcut($config?.shortcut ?? '...')}</kbd>
        </div>
        <div class="setting-item">
          <span class="label-text">Quit shortcut</span>
          <kbd>{formatShortcut($config?.quit_shortcut ?? '...')}</kbd>
        </div>
        <div class="setting-item">
          <span class="label-text">Language</span>
          {#if editingLanguage}
            <div class="inline-edit">
              <input
                type="text"
                bind:value={languageInput}
                onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && saveLanguage()}
                placeholder="auto, en, zh, ja..."
              />
              <button class="save-btn" onclick={saveLanguage}>✓</button>
              <button class="cancel-btn" onclick={() => editingLanguage = false}>✕</button>
            </div>
          {:else}
            <button class="edit-value" onclick={startEditLanguage}>{$config?.language ?? 'auto'}</button>
          {/if}
        </div>
        <div class="setting-item">
          <span class="label-text">Sound effects</span>
          <button class="toggle" class:on={$config?.sound_enabled} onclick={toggleSound}>
            {$config?.sound_enabled ? 'On' : 'Off'}
          </button>
        </div>
        <div class="setting-item">
          <span class="label-text">Recording indicator</span>
          <button class="toggle" class:on={$config?.show_recording_indicator} onclick={toggleIndicator}>
            {$config?.show_recording_indicator ? 'On' : 'Off'}
          </button>
        </div>
        <div class="setting-item">
          <span class="label-text">LLM 校正</span>
          <button class="toggle" class:on={$config?.llm_enabled} onclick={toggleLlm}>
            {$config?.llm_enabled ? 'On' : 'Off'}
          </button>
        </div>
        {#if $config?.llm_enabled}
          <div class="setting-item">
            <span class="label-text">LLM 模型</span>
            {#if editingLlmModel}
              <div class="inline-edit">
                <input
                  type="text"
                  bind:value={llmModelInput}
                  onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && saveLlmModel()}
                  placeholder="gemma3:4b"
                  style="width: 140px"
                />
                <button class="save-btn" onclick={saveLlmModel}>✓</button>
                <button class="cancel-btn" onclick={() => editingLlmModel = false}>✕</button>
              </div>
            {:else}
              <button class="edit-value" onclick={startEditLlmModel}>{$config?.llm_model ?? 'gemma3:4b'}</button>
            {/if}
          </div>
          <div class="setting-item">
            <span class="label-text">Ollama 狀態</span>
            <span class="model-status" class:loaded={llmStatus?.model_available}>
              {#if llmStatus === null}
                Checking...
              {:else if !llmStatus.available}
                Not available
              {:else if !llmStatus.model_available}
                Model not found
              {:else}
                Ready
              {/if}
            </span>
          </div>
        {/if}
        <div class="setting-item">
          <span class="label-text">Model</span>
          {#if isDownloading}
            <div class="download-progress">
              <div class="progress-bar">
                <div class="progress-fill" style="width: {downloadProgress}%"></div>
              </div>
              <span class="progress-text">{downloadProgress}%</span>
            </div>
          {:else if $modelLoaded}
            <span class="model-status loaded">Loaded</span>
          {:else}
            <span class="model-status">Not loaded</span>
            <button class="download-btn" onclick={startModelDownload}>Download</button>
          {/if}
        </div>
        {#if downloadError}
          <div class="setting-item error-text">{downloadError}</div>
        {/if}
        <p class="settings-hint">Shortcuts can be changed in config.json</p>
      </div>
    {/if}
  </section>
</main>

<style>
  :global(body) {
    margin: 0;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
    background: #1a1a2e;
    color: #e0e0e0;
  }

  main {
    display: flex;
    flex-direction: column;
    height: 100vh;
    max-width: 480px;
    margin: 0 auto;
  }

  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    background: #16213e;
    border-bottom: 1px solid #0f3460;
  }

  .status-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .status-text {
    font-size: 14px;
    font-weight: 500;
  }

  .warning {
    margin-left: auto;
    font-size: 12px;
    color: #f59e0b;
  }

  .download-btn {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 4px;
    border: 1px solid #4ade80;
    background: transparent;
    color: #4ade80;
    cursor: pointer;
    margin-left: 6px;
  }

  .download-btn:hover {
    background: rgba(74, 222, 128, 0.15);
  }

  .download-info {
    margin-left: auto;
    font-size: 12px;
    color: #60a5fa;
  }

  .download-progress {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 1;
  }

  .progress-bar {
    flex: 1;
    height: 6px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 3px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: #4ade80;
    border-radius: 3px;
    transition: width 0.3s ease;
  }

  .progress-text {
    font-size: 12px;
    color: #60a5fa;
    min-width: 36px;
    text-align: right;
  }

  .error-text {
    color: #f87171;
    font-size: 12px;
  }

  .error-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    background: #7f1d1d;
    color: #fca5a5;
    font-size: 13px;
  }

  .error-bar button {
    margin-left: auto;
    background: none;
    border: none;
    color: #fca5a5;
    font-size: 16px;
    cursor: pointer;
  }

  nav {
    display: flex;
    border-bottom: 1px solid #0f3460;
  }

  nav button {
    flex: 1;
    padding: 10px;
    background: none;
    border: none;
    color: #94a3b8;
    font-size: 13px;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    transition: all 0.2s;
  }

  nav button:hover {
    color: #e0e0e0;
    background: #16213e;
  }

  nav button.active {
    color: #60a5fa;
    border-bottom-color: #60a5fa;
  }

  .content {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
  }

  .status-panel {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 24px;
    padding-top: 32px;
  }

  .big-status {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
  }

  .big-dot {
    width: 48px;
    height: 48px;
    border-radius: 50%;
    transition: background-color 0.3s;
  }

  .big-status h2 {
    margin: 0;
    font-size: 20px;
    font-weight: 500;
  }

  .last-result {
    width: 100%;
    padding: 12px;
    background: #16213e;
    border-radius: 8px;
  }

  .last-result p {
    margin: 4px 0 0;
    font-size: 15px;
    line-height: 1.4;
  }

  .shortcut-hint {
    color: #94a3b8;
    font-size: 13px;
  }

  kbd {
    padding: 2px 6px;
    background: #0f3460;
    border-radius: 4px;
    font-family: inherit;
    font-size: 12px;
  }

  .history-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 12px;
  }

  .history-header h3 {
    margin: 0;
    font-size: 15px;
  }

  .clear-btn {
    padding: 4px 12px;
    background: #7f1d1d;
    color: #fca5a5;
    border: none;
    border-radius: 4px;
    font-size: 12px;
    cursor: pointer;
  }

  .empty {
    color: #64748b;
    text-align: center;
    padding: 32px 0;
  }

  .history-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .history-list li {
    padding: 10px;
    background: #16213e;
    border-radius: 6px;
  }

  .entry-meta {
    display: flex;
    gap: 8px;
    font-size: 11px;
    color: #64748b;
    margin-bottom: 4px;
  }

  .entry-text {
    margin: 0;
    font-size: 14px;
    line-height: 1.4;
  }

  .settings-panel {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .setting-item {
    display: flex;
    justify-content: space-between;
    padding: 10px;
    background: #16213e;
    border-radius: 6px;
  }

  .label-text {
    color: #94a3b8;
    font-size: 13px;
  }

  .last-result .label-text {
    font-size: 11px;
    text-transform: uppercase;
  }

  .setting-item span {
    font-size: 13px;
  }

  .edit-value {
    background: none;
    border: 1px solid transparent;
    color: #e0e0e0;
    font-size: 13px;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 4px;
  }

  .edit-value:hover {
    border-color: #334155;
    background: #0f3460;
  }

  .inline-edit {
    display: flex;
    gap: 4px;
    align-items: center;
  }

  .inline-edit input {
    width: 80px;
    padding: 2px 6px;
    background: #0f3460;
    border: 1px solid #334155;
    border-radius: 4px;
    color: #e0e0e0;
    font-size: 13px;
  }

  .save-btn, .cancel-btn {
    background: none;
    border: none;
    cursor: pointer;
    font-size: 14px;
    padding: 2px 4px;
  }

  .save-btn { color: #22c55e; }
  .cancel-btn { color: #94a3b8; }

  .toggle {
    padding: 4px 12px;
    border: 1px solid #334155;
    border-radius: 12px;
    font-size: 12px;
    cursor: pointer;
    background: #1e293b;
    color: #94a3b8;
    transition: all 0.2s;
  }

  .toggle.on {
    background: #166534;
    border-color: #22c55e;
    color: #4ade80;
  }

  .model-status {
    font-size: 13px;
    color: #94a3b8;
  }

  .model-status.loaded {
    color: #4ade80;
  }

  .llm-badge {
    background: #8b5cf6;
    color: white;
    padding: 1px 6px;
    border-radius: 3px;
    font-size: 10px;
    margin-left: 4px;
    text-transform: uppercase;
  }

  .settings-hint {
    color: #475569;
    font-size: 11px;
    text-align: center;
    margin-top: 8px;
  }

  .setting-item kbd {
    background: #0f3460;
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 12px;
    font-family: inherit;
  }
</style>
