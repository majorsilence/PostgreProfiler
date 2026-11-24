<script setup lang="ts">
import { computed, ref } from 'vue'
import { compact, formatTick, maxValue, nearest, niceTicks, timeTicks, type Series } from '../lib/chart'

const props = withDefaults(
  defineProps<{
    series: Series[]
    kind?: 'line' | 'bars'
    height?: number
    /** Visible time window, ms since the epoch. */
    from: number
    to: number
    /** Width of one bucket in ms (for bars and for hovering). */
    bucketMs?: number
    format?: (v: number) => string
    unit?: string
  }>(),
  { kind: 'line', height: 240, bucketMs: 0, format: undefined, unit: '' },
)

const W = 900
const pad = { l: 58, r: 14, t: 12, b: 28 }
const plotW = W - pad.l - pad.r
const plotH = computed(() => props.height - pad.t - pad.b)
const hidden = ref<Set<string>>(new Set())

const shown = computed(() => props.series.filter((s) => !hidden.value.has(s.name) && s.points.length))
const stacked = computed(() => props.kind === 'bars')
const yTicks = computed(() => niceTicks(0, Math.max(maxValue(shown.value, stacked.value), 1e-9) * 1.05, 5))
const yMax = computed(() => yTicks.value[yTicks.value.length - 1] || 1)
const span = computed(() => Math.max(1, props.to - props.from))
const xTicks = computed(() => timeTicks(props.from, props.to, 6))
const fmt = (v: number) => (props.format ? props.format(v) : compact(v))

const x = (t: number) => pad.l + ((t - props.from) / span.value) * plotW
const y = (v: number) => pad.t + (1 - v / yMax.value) * plotH.value

const paths = computed(() =>
  shown.value.map((s) => {
    const pts = [...s.points].sort((a, b) => a.t - b.t)
    return { name: s.name, color: s.color, d: pts.map((p, i) => `${i ? 'L' : 'M'}${x(p.t).toFixed(1)} ${y(p.v).toFixed(1)}`).join(' '), single: pts.length === 1 ? pts[0] : null }
  }),
)

const barW = computed(() => Math.max(2, ((props.bucketMs || span.value / 60) / span.value) * plotW * 0.82))

const bars = computed(() => {
  if (!stacked.value) return []
  const byT = new Map<number, { name: string; color: string; v: number }[]>()
  for (const s of shown.value) for (const p of s.points) byT.set(p.t, [...(byT.get(p.t) ?? []), { name: s.name, color: s.color, v: p.v }])
  const out: { key: string; x: number; y: number; h: number; w: number; color: string }[] = []
  for (const [t, parts] of byT) {
    let acc = 0
    for (const p of parts) {
      const top = y(acc + p.v)
      out.push({ key: `${t}-${p.name}`, x: x(t) - barW.value / 2, y: top, h: Math.max(0, y(acc) - top), w: barW.value, color: p.color })
      acc += p.v
    }
  }
  return out
})

const hover = ref<{ left: number; items: { name: string; color: string; v: number; t: number }[] } | null>(null)

function move(e: MouseEvent) {
  const el = e.currentTarget as SVGElement
  const rect = el.getBoundingClientRect()
  const px = ((e.clientX - rect.left) / rect.width) * W
  const t = props.from + ((px - pad.l) / plotW) * span.value
  const tol = props.bucketMs ? props.bucketMs / 2 : span.value / 40
  const items = nearest(shown.value, t, tol)
  hover.value = items.length ? { left: ((x(items[0].t)) / W) * 100, items } : null
}

function toggle(name: string) {
  const next = new Set(hidden.value)
  if (next.has(name)) next.delete(name)
  else next.add(name)
  hidden.value = next
}

const empty = computed(() => !props.series.some((s) => s.points.length))
const when = (t: number) => new Date(t).toLocaleString(undefined, { dateStyle: 'short', timeStyle: 'short' })
</script>

<template>
  <figure class="chart">
    <div class="chart-box" @mouseleave="hover = null">
      <svg :viewBox="`0 0 ${W} ${height}`" role="img" :aria-label="`Chart of ${series.map((s) => s.name).join(', ')}`" @mousemove="move">
        <g class="grid-lines">
          <g v-for="v in yTicks" :key="'y' + v">
            <line :x1="pad.l" :x2="W - pad.r" :y1="y(v)" :y2="y(v)" />
            <text :x="pad.l - 6" :y="y(v) + 4" text-anchor="end">{{ fmt(v) }}</text>
          </g>
          <g v-for="t in xTicks" :key="'x' + t">
            <line class="tick" :x1="x(t)" :x2="x(t)" :y1="height - pad.b" :y2="height - pad.b + 4" />
            <text :x="x(t)" :y="height - 8" text-anchor="middle">{{ formatTick(t, span) }}</text>
          </g>
        </g>
        <template v-if="stacked">
          <rect v-for="b in bars" :key="b.key" :x="b.x" :y="b.y" :width="b.w" :height="b.h" :fill="b.color" />
        </template>
        <template v-else>
          <g v-for="p in paths" :key="p.name">
            <path :d="p.d" :stroke="p.color" fill="none" stroke-width="2" stroke-linejoin="round" />
            <circle v-if="p.single" :cx="x(p.single.t)" :cy="y(p.single.v)" r="3.5" :fill="p.color" />
          </g>
        </template>
        <line v-if="hover" class="cursor" :x1="(hover.left / 100) * W" :x2="(hover.left / 100) * W" :y1="pad.t" :y2="height - pad.b" />
      </svg>
      <div v-if="hover" class="tip card" :style="{ left: Math.min(hover.left, 70) + '%' }">
        <b>{{ when(hover.items[0].t) }}</b>
        <div v-for="i in hover.items" :key="i.name"><i class="dot" :style="{ background: i.color }"></i>{{ i.name }}: {{ fmt(i.v) }}{{ unit }}</div>
      </div>
      <p v-if="empty" class="empty muted">No data in this period.</p>
    </div>
    <figcaption v-if="series.length > 1 || series[0]?.name" class="legend">
      <button v-for="s in series" :key="s.name" class="legend-item" :class="{ off: hidden.has(s.name) }" :aria-pressed="!hidden.has(s.name)" @click="toggle(s.name)">
        <i class="dot" :style="{ background: s.color }"></i>{{ s.name }}
      </button>
    </figcaption>
  </figure>
</template>
