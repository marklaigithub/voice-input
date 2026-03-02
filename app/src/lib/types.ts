export interface AppConfig {
  model_path: string
  shortcut: string
  quit_shortcut: string
  language: string
  sound_enabled: boolean
  ffmpeg_path: string | null
  max_history: number
  llm_enabled: boolean
  llm_model: string
  llm_endpoint: string
  show_recording_indicator: boolean
  indicator_x: number | null
  indicator_y: number | null
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

export type AppState = 'idle' | 'recording' | 'transcribing' | 'correcting' | 'loading'
