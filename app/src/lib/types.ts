export interface AppConfig {
  model_path: string
  shortcut: string
  language: string
  sound_enabled: boolean
  ffmpeg_path: string | null
  max_history: number
}

export interface AppStatus {
  model_loaded: boolean
  is_recording: boolean
  is_busy: boolean
  config: AppConfig
}

export interface HistoryEntry {
  timestamp: string
  text: string
  source: { PressToTalk: null } | { File: string }
  duration_secs: number
}

export type AppState = 'idle' | 'recording' | 'transcribing' | 'loading'
