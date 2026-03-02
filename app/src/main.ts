import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { mount } from 'svelte'

const label = getCurrentWebviewWindow().label

if (label === 'indicator') {
  const { default: Indicator } = await import('./Indicator.svelte')
  mount(Indicator, { target: document.getElementById('app')! })
} else {
  const { default: App } = await import('./App.svelte')
  mount(App, { target: document.getElementById('app')! })
}
