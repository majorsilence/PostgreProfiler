<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import Chart from '../components/Chart.vue'
import RangePicker from '../components/RangePicker.vue'
import StoreNotice from '../components/StoreNotice.vue'
import { palette, type Series } from '../lib/chart'
import { formatDuration, oneLine } from '../lib/format'
import { store, type Wait, type WaitPoint } from '../lib/qs'
import { bucketSeconds } from '../lib/range'
import { range, startAutoRefresh } from '../lib/rangeState'
import { queryPath } from '../lib/storeCommon'

const router = useRouter()
const waits = ref<Wait[]>([])
const points = ref<WaitPoint[]>([])
const error = ref('')
const loading = ref(true)
const showCpu = ref(false)
const stop = startAutoRefresh()
onBeforeUnmount(stop)

const bucket = computed(() => bucketSeconds(range.value))

let seq = 0
async function load() {
  const mine = ++seq
  loading.value = true
  try {
    const [w, p] = await Promise.all([store.waits(range.value, 100), store.waitSeries(range.value, bucket.value)])
    if (mine !== seq) return
    waits.value = w
    points.value = p
    error.value = ''
  } catch (e) {
    if (mine === seq) error.value = e instanceof Error ? e.message : 'could not load'
  } finally {
    if (mine === seq) loading.value = false
  }
}
watch([range], load, { immediate: true })

// "cpu" means "not waiting" and usually dwarfs everything else, so it is hidden by default
const categories = computed(() => {
  const totals = new Map<string, number>()
  for (const p of points.value) totals.set(p.category, (totals.get(p.category) ?? 0) + p.samples)
  return [...totals.entries()].filter(([c]) => showCpu.value || c !== 'cpu').sort((a, b) => b[1] - a[1])
})

const series = computed<Series[]>(() =>
  categories.value.map(([name], i) => ({
    name,
    color: palette[i % palette.length],
    points: points.value.filter((p) => p.category === name).map((p) => ({ t: new Date(p.bucket_start).getTime(), v: p.samples })),
  })),
)

const rows = computed(() => waits.value.filter((w) => showCpu.value || w.category !== 'cpu'))
const total = computed(() => categories.value.reduce((s, [, n]) => s + n, 0))
</script>

<template>
  <section>
    <div class="toolbar">
      <h1>Wait statistics</h1>
      <RangePicker />
    </div>
    <StoreNotice />
    <p class="muted">
      What running statements waited for, sampled every <code>pg_profiler.wait_sample_ms</code> (10 ms by default), so a
      sample stands for that much time. Statements shorter than the period show nothing.
    </p>
    <div class="filters">
      <label class="check"><input v-model="showCpu" type="checkbox" /> include "cpu" (not waiting)</label>
      <span v-if="loading" class="muted">Loading…</span>
    </div>
    <p v-if="error" class="error" role="alert">{{ error }}</p>

    <div class="card">
      <h2 class="inline-h">Waits over time</h2>
      <Chart :series="series" kind="bars" :from="range.from.getTime()" :to="range.to.getTime()" :bucket-ms="bucket * 1000" unit=" samples" />
      <p v-if="categories.length" class="muted">
        Total: {{ total }} samples ({{ formatDuration(total * 10) }} at a 10 ms period)
      </p>
    </div>

    <h2>By statement</h2>
    <div class="grid-wrap" v-if="rows.length">
      <table class="grid">
        <thead>
          <tr><th>Statement</th><th>Waiting for</th><th class="r">Samples</th><th>Share of the statement's time</th></tr>
        </thead>
        <tbody>
          <tr v-for="(w, i) in rows" :key="i" tabindex="0" @click="router.push(queryPath(w.dbid, w.queryid))" @keydown.enter="router.push(queryPath(w.dbid, w.queryid))">
            <td class="stmt" :title="w.query ?? ''">{{ oneLine(w.query, 120) || '(text not available)' }}</td>
            <td><span class="kind">{{ w.category }}</span></td>
            <td class="r">{{ w.samples }}</td>
            <td class="cv"><div class="bar"><i :style="{ width: w.pct_of_query + '%' }"></i></div>{{ w.pct_of_query.toFixed(0) }} %</td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-else-if="!loading && !error" class="muted pad">No waits were sampled in this period.</p>
  </section>
</template>
