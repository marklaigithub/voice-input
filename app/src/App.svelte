<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { onMount } from 'svelte'
  import { appState, modelLoaded, config, history, lastTranscription, errorMessage } from './lib/store'
  import type { AppStatus, HistoryEntry } from './lib/types'

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

      // If model not loaded, try to init
      if (!status.model_loaded) {
        const hasModel: boolean = await invoke('check_model')
        if (hasModel) {
          $appState = 'loading'
          await invoke('init_whisper')
          $modelLoaded = true
          $appState = 'idle'
        }
      }
    } catch (e) {
      $errorMessage = String(e)
      $appState = 'idle'
    }

    // Listen for shortcut events (press-to-talk)
    await listen<string>('shortcut-event', async (event) => {
      if (event.payload === 'pressed' && $appState === 'idle' && $modelLoaded) {
        try {
          $appState = 'recording'
          await invoke('start_recording')
        } catch (e) {
          $errorMessage = String(e)
          $appState = 'idle'
        }
      } else if (event.payload === 'released' && $appState === 'recording') {
        try {
          $appState = 'transcribing'
          const text: string = await invoke('stop_recording_and_transcribe')
          $lastTranscription = text
          $appState = 'idle'
          // Refresh history
          const h: HistoryEntry[] = await invoke('get_history')
          $history = h
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
      case 'loading': return 'Loading model...'
      default: return state
    }
  }

  function stateColor(state: string): string {
    switch (state) {
      case 'recording': return '#ef4444'
      case 'transcribing': return '#f59e0b'
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
    if ('PressToTalk' in source) return 'PTT'
    if ('File' in source) return source.File
    return '?'
  }
</script>

<main>
  <header>
    <div class="status-dot" style="background-color: {stateColor($appState)}"></div>
    <span class="status-text">{stateLabel($appState)}</span>
    {#if !$modelLoaded}
      <span class="warning">Model not loaded</span>
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
          <div class="big-dot" style="background-color: {stateColor($appState)}"></div>
          <h2>{stateLabel($appState)}</h2>
        </div>
        {#if $lastTranscription}
          <div class="last-result">
            <span class="label-text">Last transcription:</span>
            <p>{$lastTranscription}</p>
          </div>
        {/if}
        <div class="shortcut-hint">
          <p>Press <kbd>{$config?.shortcut ?? 'Cmd+Shift+Space'}</kbd> to talk</p>
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
        <p class="placeholder">Settings panel (coming soon)</p>
        <div class="setting-item">
          <span class="label-text">Shortcut</span>
          <span>{$config?.shortcut ?? '...'}</span>
        </div>
        <div class="setting-item">
          <span class="label-text">Language</span>
          <span>{$config?.language ?? 'auto'}</span>
        </div>
        <div class="setting-item">
          <span class="label-text">Sound effects</span>
          <span>{$config?.sound_enabled ? 'On' : 'Off'}</span>
        </div>
        <div class="setting-item">
          <span class="label-text">Model</span>
          <span>{$modelLoaded ? 'Loaded' : 'Not loaded'}</span>
        </div>
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

  .last-result label {
    font-size: 11px;
    color: #94a3b8;
    text-transform: uppercase;
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

  .placeholder {
    color: #64748b;
    font-size: 13px;
    text-align: center;
  }
</style>
