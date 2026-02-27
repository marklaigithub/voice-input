import { writable } from 'svelte/store'
import type { AppConfig, AppState, HistoryEntry } from './types'

export const appState = writable<AppState>('loading')
export const modelLoaded = writable(false)
export const config = writable<AppConfig | null>(null)
export const history = writable<HistoryEntry[]>([])
export const lastTranscription = writable<string>('')
export const errorMessage = writable<string>('')
