<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import { listen } from '@tauri-apps/api/event'
  import { onMount } from 'svelte'
  import AudioWaveform from './lib/AudioWaveform.svelte'

  let audioLevel = $state(0)
  let elapsedSeconds = $state(0)
  let isRecording = $state(false)

  let audioLevelInterval: ReturnType<typeof setInterval> | null = null
  let timerInterval: ReturnType<typeof setInterval> | null = null

  function startPolling() {
    isRecording = true
    elapsedSeconds = 0
    audioLevel = 0

    audioLevelInterval = setInterval(async () => {
      try {
        audioLevel = await invoke<number>('get_audio_level')
      } catch {
        audioLevel = 0
      }
    }, 80)

    timerInterval = setInterval(() => {
      elapsedSeconds++
    }, 1000)
  }

  function stopPolling() {
    isRecording = false
    if (audioLevelInterval) {
      clearInterval(audioLevelInterval)
      audioLevelInterval = null
    }
    if (timerInterval) {
      clearInterval(timerInterval)
      timerInterval = null
    }
    audioLevel = 0
    elapsedSeconds = 0
  }

  function formatTime(secs: number): string {
    const m = Math.floor(secs / 60)
    const s = secs % 60
    return `${m}:${String(s).padStart(2, '0')}`
  }

  onMount(async () => {
    await listen('recording-started', () => {
      startPolling()
    })

    await listen('recording-stopped', () => {
      stopPolling()
    })
  })
</script>

<div class="indicator" data-tauri-drag-region>
  {#if isRecording}
    <div class="rec-dot"></div>
    <AudioWaveform level={audioLevel} />
    <span class="timer">{formatTime(elapsedSeconds)}</span>
  {/if}
</div>

<style>
  :global(html), :global(body) {
    margin: 0;
    padding: 0;
    background: transparent;
    overflow: hidden;
  }

  .indicator {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 12px;
    background: rgba(26, 26, 46, 0.88);
    border-radius: 12px;
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
    border: 1px solid rgba(255, 255, 255, 0.08);
    cursor: grab;
    user-select: none;
    height: 48px;
    box-sizing: border-box;
  }

  .rec-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: #ef4444;
    flex-shrink: 0;
    animation: pulse 1.2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  .timer {
    font-family: -apple-system, BlinkMacSystemFont, 'SF Mono', 'Menlo', monospace;
    font-size: 14px;
    font-variant-numeric: tabular-nums;
    color: #e0e0e0;
    min-width: 32px;
    text-align: right;
  }
</style>
