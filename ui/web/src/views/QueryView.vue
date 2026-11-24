<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import Chart from '../components/Chart.vue'
import PlanDiffView from '../components/PlanDiffView.vue'
import PlanView from '../components/PlanView.vue'
import RangePicker from '../components/RangePicker.vue'
import StoreNotice from '../components/StoreNotice.vue'
import { compact, palette, type Series } from '../lib/chart'
import { formatDateTime, formatMs } from '../lib/format'
import { store, type PlanInfo, type PlanSample, type PlanText, type QueryInterval } from '../lib/qs'
import { bucketSeconds } from '../lib/range'
import { range, startAutoRefresh } from '../lib/rangeState'

const route = useRoute()
const dbid = computed(() => String(route.params.dbid))
const queryid = computed(() => String(route.params.queryid))

const text = ref<string | null>(null)
const history = ref<QueryInterval[]>([])
const plans = ref<PlanInfo[]>([])
const samples = ref<PlanSample[]>([])
const error = ref('')
const loading = ref(true)
const metric = ref<'avg_exec_ms' | 'p95_exec_ms' | 'max_exec_ms' | 'calls' | 'errors' | 'rows' | 'shared_blks_read' | 'temp_blks_written'>('avg_exec_ms')
const stop = startAutoRefresh()
onBeforeUnmount(stop)

const choices = [
  { id: 'avg_exec_ms', label: 'Average time (ms)', bars: false },
  { id: 'p95_exec_ms', label: 'p95 time (ms)', bars: false },
  { id: 'max_exec_ms', label: 'Slowest run (ms)', bars: false },
  { id: 'calls', label: 'Executions', bars: true },
  { id: 'errors', label: 'Errors', bars: true },
  { id: 'rows', label: 'Rows', bars: true },
  { id: 'shared_blks_read', label: 'Blocks read', bars: true },
  { id: 'temp_blks_written', label: 'Temp blocks written', bars: true },
] as const
const choice = computed(() => choices.find((c) => c.id === metric.value) ?? choices[0])
const bucket = computed(() => bucketSeconds(range.value))

const planIds = computed(() => [...new Set(history.value.map((h) => String(h.plan_id)))])
const colorOf = (id: string) => palette[Math.max(0, planIds.value.indexOf(id)) % palette.length]
const short = (id: string | number) => String(id).slice(-6)

const series = computed<Series[]>(() =>
  planIds.value.map((id) => ({
    name: id === '0' ? 'no plan' : `plan …${short(id)}`,
    color: colorOf(id),
    points: history.value
      .filter((h) => String(h.plan_id) === id && h[metric.value] !== null)
      .map((h) => ({ t: new Date(h.interval_start).getTime(), v: Number(h[metric.value]) })),
  })),
)

// per-plan numbers over the selected period
const perPlan = computed(() => {
  const m = new Map<string, { calls: number; ms: number; p95: number; errors: number }>()
  for (const h of history.value) {
    const k = String(h.plan_id)
    const e = m.get(k) ?? { calls: 0, ms: 0, p95: 0, errors: 0 }
    e.calls += h.calls
    e.ms += (h.avg_exec_ms ?? 0) * h.calls
    e.p95 = Math.max(e.p95, h.p95_exec_ms ?? 0)
    e.errors += h.errors
    m.set(k, e)
  }
  return m
})

let seq = 0
async function load() {
  const mine = ++seq
  loading.value = true
  try {
    const [q, p] = await Promise.all([store.query(dbid.value, queryid.value, range.value), store.plans(dbid.value, queryid.value)])
    if (mine !== seq) return
    text.value = q.query
    history.value = q.history
    plans.value = p.plans
    samples.value = p.samples
    error.value = ''
  } catch (e) {
    if (mine === seq) error.value = e instanceof Error ? e.message : 'could not load'
  } finally {
    if (mine === seq) loading.value = false
  }
}
watch([range, dbid, queryid], load, { immediate: true })

// ---- plans
const shown = ref<{ text: string; note: string } | null>(null)
const selected = ref<string[]>([])
const diff = ref<{ a: string; b: string; labelA: string; labelB: string } | null>(null)
const planError = ref('')

watch([dbid, queryid], () => {
  shown.value = null
  diff.value = null
  selected.value = []
})

async function fetchPlan(planId: string | number, at?: string): Promise<PlanText | null> {
  planError.value = ''
  try {
    const p = await store.plan(dbid.value, queryid.value, planId, at)
    if (!p) planError.value = 'This plan is no longer stored (the plan buffer is small and the history keeps a limited number per query).'
    return p
  } catch (e) {
    planError.value = e instanceof Error ? e.message : 'could not load the plan'
    return null
  }
}

async function viewPlan(p: PlanInfo) {
  diff.value = null
  const t = await fetchPlan(p.plan_id)
  if (t) shown.value = { text: t.plan, note: `plan …${short(p.plan_id)} · first stored ${formatDateTime(t.ts)}` }
}

async function viewSample(s: PlanSample) {
  diff.value = null
  const t = await fetchPlan(s.plan_id, s.ts)
  if (t) shown.value = { text: t.plan, note: `${s.reason} run of ${formatMs(s.exec_time_ms)} ms at ${formatDateTime(s.ts)}, with actual rows` }
}

function toggle(id: string) {
  const i = selected.value.indexOf(id)
  if (i >= 0) selected.value.splice(i, 1)
  else selected.value = [...selected.value.slice(-1), id] // keep the two most recent picks
}

async function compare() {
  if (selected.value.length !== 2) return
  const [a, b] = await Promise.all(selected.value.map((id) => fetchPlan(id)))
  if (a && b) {
    shown.value = null
    diff.value = { a: a.plan, b: b.plan, labelA: `plan …${short(selected.value[0])}`, labelB: `plan …${short(selected.value[1])}` }
  }
}

const sorted = computed(() => [...history.value].sort((x, y) => y.interval_start.localeCompare(x.interval_start)))
</script>

<template>
  <section>
    <div class="toolbar">
      <h1>Query {{ queryid }}</h1>
      <RangePicker />
    </div>
    <StoreNotice />
    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <pre class="sql" v-if="text">{{ text }}</pre>
    <p v-else-if="!loading" class="muted">The text of this statement is not available (it was not cached when the interval closed).</p>

    <div class="card">
      <div class="toolbar">
        <h2 class="inline-h">Over time</h2>
        <select v-model="metric" aria-label="Metric">
          <option v-for="c in choices" :key="c.id" :value="c.id">{{ c.label }}</option>
        </select>
        <span class="muted">one line per plan · a new plan shows up as a new line</span>
      </div>
      <Chart :series="series" :kind="choice.bars ? 'bars' : 'line'" :from="range.from.getTime()" :to="range.to.getTime()" :bucket-ms="bucket * 1000" />
    </div>

    <h2>Plans</h2>
    <p class="muted">Pick two plans to compare them, or view one.</p>
    <div class="grid-wrap" v-if="plans.length">
      <table class="grid compact">
        <thead>
          <tr><th></th><th>Plan</th><th>First seen</th><th>Last seen</th><th class="r">Executions in period</th><th class="r">Average (ms)</th><th class="r">p95 (ms)</th><th></th></tr>
        </thead>
        <tbody>
          <tr v-for="p in plans" :key="String(p.plan_id)">
            <td><input type="checkbox" :checked="selected.includes(String(p.plan_id))" :aria-label="'Select plan ' + short(p.plan_id)" @change="toggle(String(p.plan_id))" /></td>
            <td><i class="dot" :style="{ background: colorOf(String(p.plan_id)) }"></i><code>…{{ short(p.plan_id) }}</code></td>
            <td>{{ formatDateTime(p.first_seen) }}</td>
            <td>{{ formatDateTime(p.last_seen) }}</td>
            <td class="r">{{ compact(perPlan.get(String(p.plan_id))?.calls ?? 0) }}</td>
            <td class="r">{{ perPlan.get(String(p.plan_id))?.calls ? formatMs(perPlan.get(String(p.plan_id))!.ms / perPlan.get(String(p.plan_id))!.calls) : '' }}</td>
            <td class="r">{{ perPlan.get(String(p.plan_id))?.p95 ? formatMs(perPlan.get(String(p.plan_id))!.p95) : '' }}</td>
            <td class="actions"><button @click="viewPlan(p)">View</button></td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-else-if="!loading" class="muted">No plan is stored for this statement yet.</p>
    <p><button :disabled="selected.length !== 2" @click="compare">Compare the two selected plans</button></p>
    <p v-if="planError" class="notice">{{ planError }}</p>

    <PlanDiffView v-if="diff" :a="diff.a" :b="diff.b" :label-a="diff.labelA" :label-b="diff.labelB" />
    <PlanView v-else-if="shown" :text="shown.text" :note="shown.note" />

    <template v-if="samples.length">
      <h2>Slow, spilling or mis-estimated runs</h2>
      <p class="muted">Runs that were stored with actual row counts (see <code>pg_profiler.plan_sample_rate</code>).</p>
      <table class="grid compact">
        <thead><tr><th>When</th><th>Plan</th><th>Why</th><th class="r">Time (ms)</th><th class="r">Worst estimate</th><th class="r">Temp blocks</th><th></th></tr></thead>
        <tbody>
          <tr v-for="s in samples" :key="s.ts + s.reason">
            <td>{{ formatDateTime(s.ts) }}</td>
            <td><code>…{{ short(s.plan_id) }}</code></td>
            <td>{{ s.reason }}</td>
            <td class="r">{{ formatMs(s.exec_time_ms) }}</td>
            <td class="r">{{ s.max_misestimate ? s.max_misestimate.toFixed(0) + '×' : '' }}</td>
            <td class="r">{{ s.temp_blks_written || '' }}</td>
            <td class="actions"><button @click="viewSample(s)">View with actual rows</button></td>
          </tr>
        </tbody>
      </table>
    </template>

    <h2>Intervals</h2>
    <div class="grid-wrap" v-if="sorted.length">
      <table class="grid compact">
        <thead>
          <tr><th>Interval</th><th>Plan</th><th class="r">Executions</th><th class="r">Errors</th><th class="r">Average (ms)</th><th class="r">Std dev</th><th class="r">Min</th><th class="r">Max</th><th class="r">p95</th><th class="r">Rows</th><th class="r">Reads</th></tr>
        </thead>
        <tbody>
          <tr v-for="h in sorted.slice(0, 200)" :key="h.interval_start + h.plan_id">
            <td>{{ formatDateTime(h.interval_start) }}</td>
            <td><i class="dot" :style="{ background: colorOf(String(h.plan_id)) }"></i>…{{ short(h.plan_id) }}</td>
            <td class="r">{{ h.calls }}</td>
            <td class="r" :class="{ bad: h.errors }">{{ h.errors || '' }}</td>
            <td class="r">{{ formatMs(h.avg_exec_ms) }}</td>
            <td class="r">{{ formatMs(h.stddev_exec_ms) }}</td>
            <td class="r">{{ formatMs(h.min_exec_ms) }}</td>
            <td class="r">{{ formatMs(h.max_exec_ms) }}</td>
            <td class="r">{{ formatMs(h.p95_exec_ms) }}</td>
            <td class="r">{{ h.rows }}</td>
            <td class="r">{{ h.shared_blks_read }}</td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-else-if="!loading" class="muted pad">This query has no statistics in the selected period.</p>
  </section>
</template>
