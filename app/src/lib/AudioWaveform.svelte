<script lang="ts">
  let { level = 0 }: { level: number } = $props()

  // Smoothed level for CSS transition
  let smoothLevel = $derived(Math.max(0, Math.min(1, level)))

  // Wave parameters: each wave has different frequency, phase offset, and opacity
  const waves = [
    { freq: 1.2, phase: 0, opacity: 0.9, color: '#ef4444' },
    { freq: 1.8, phase: 0.4, opacity: 0.5, color: '#f87171' },
    { freq: 2.5, phase: 0.8, opacity: 0.3, color: '#fca5a5' },
  ]

  // Animate time for wave motion
  let time = $state(0)
  let animFrame: number | null = null

  $effect(() => {
    function tick() {
      time += 0.04
      animFrame = requestAnimationFrame(tick)
    }
    animFrame = requestAnimationFrame(tick)
    return () => {
      if (animFrame !== null) cancelAnimationFrame(animFrame)
    }
  })

  // Generate SVG path for a sine wave
  function wavePath(freq: number, phase: number, amplitude: number): string {
    const width = 200
    const height = 60
    const mid = height / 2
    const points: string[] = []
    const steps = 50

    for (let i = 0; i <= steps; i++) {
      const x = (i / steps) * width
      const normalizedX = (i / steps) * Math.PI * 2 * freq
      const y = mid + Math.sin(normalizedX + phase + time) * amplitude
      points.push(i === 0 ? `M ${x} ${y}` : `L ${x} ${y}`)
    }

    return points.join(' ')
  }
</script>

<div class="waveform-container">
  <svg viewBox="0 0 200 60" preserveAspectRatio="none">
    {#each waves as wave}
      <path
        d={wavePath(wave.freq, wave.phase, smoothLevel * 22 + 1)}
        fill="none"
        stroke={wave.color}
        stroke-width="2"
        opacity={wave.opacity}
        stroke-linecap="round"
      />
    {/each}
  </svg>
</div>

<style>
  .waveform-container {
    width: 120px;
    height: 48px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  svg {
    width: 100%;
    height: 100%;
  }

  path {
    transition: d 0.08s ease-out;
  }
</style>
