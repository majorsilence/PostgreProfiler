<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import RangePicker from '../components/RangePicker.vue'
import StoreNotice from '../components/StoreNotice.vue'
import { compact } from '../lib/chart'
import { formatDuration, formatMs, oneLine } from '../lib/format'
import { store, type Regressed } from '../lib/qs'
import { describe, previous } from '../lib/range'
import { range, startAutoRefresh } from '../lib/rangeState'
import { queryPath } from '../lib/storeCommon'

const router = useRouter()
const rows = ref<Regressed[]>([])
const error = ref('')
const loading = ref(true)
const minCalls = ref(10)
const minRatio = ref(1.5)
const minZ = ref(3)
const stop = startAutoRefresh()
onBeforeUnmount(stop)

const baseline = computed(() => previous(range.value))

let seq = 0
async function load() {
  const mine = ++seq
  loading.value = true
  try {
    const r = await store.regressed(range.value, baseline.value, minCalls.value, minRatio.value, minZ.value)
    if (mine !== seq) return
    rows.value = r
    error.value = ''
  } catch (e) {
    if (mine === seq) error.value = e instanceof Error ? e.message : 'could not load'
  } finally {
    if (mine === seq) loading.value = false
  }
}
watch([range, minCalls, minRatio, minZ], load, { immediate: true })

const addedTotal = computed(() => rows.value.reduce((s, r) => s + r.added_time_ms, 0))
const ids = (a: (string | number)[]) => a.map((x) => String(x).slice(-6)).join(', ')
</script>

<template>
  <section>
    <div class="toolbar">
      <h1>Regressed queries</h1>
      <RangePicker />
    </div>
    <StoreNotice />
    <p class="muted">
      Queries whose average execution time in the selected period is clearly higher than in the period of the same length
      before it ({{ describe(baseline) }}). A query is listed when the average grew by at least the ratio and the
      difference is statistically significant (Welch's t statistic, from the sums and sums of squares kept per interval).
    </p>
    <div class="filters">
      <label class="inline">At least <input v-model.number="minCalls" type="number" min="1" class="narrow" /> executions in both periods</label>
      <label class="inline">slower by at least <input v-model.number="minRatio" type="number" min="1" step="0.1" class="narrow" /> ×</label>
      <label class="inline">significance (z) at least <input v-model.number="minZ" type="number" min="0" step="0.5" class="narrow" /></label>
      <span v-if="loading" class="muted">Loading…</span>
    </div>
    <p v-if="error" class="error" role="alert">{{ error }}</p>

    <div class="grid-wrap" v-if="rows.length">
      <table class="grid">
        <thead>
          <tr>
            <th>Statement</th>
            <th class="r">Before (ms)</th>
            <th class="r">Now (ms)</th>
            <th class="r">Slower</th>
            <th class="r">z</th>
            <th class="r">Added time</th>
            <th class="r">Executions (before / now)</th>
            <th>Plan</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in rows" :key="r.dbid + ':' + r.queryid" tabindex="0" @click="router.push(queryPath(r.dbid, r.queryid))" @keydown.enter="router.push(queryPath(r.dbid, r.queryid))">
            <td class="stmt" :title="r.query ?? ''">{{ oneLine(r.query, 140) || '(text not available)' }}</td>
            <td class="r">{{ formatMs(r.baseline_avg_ms) }}</td>
            <td class="r bad"><b>{{ formatMs(r.recent_avg_ms) }}</b></td>
            <td class="r">{{ r.ratio.toFixed(1) }}×</td>
            <td class="r">{{ r.z_score === null ? '' : r.z_score > 999 ? '∞' : r.z_score.toFixed(1) }}</td>
            <td class="r">{{ formatDuration(r.added_time_ms) }}</td>
            <td class="r">{{ compact(r.baseline_calls) }} / {{ compact(r.recent_calls) }}</td>
            <td>
              <span v-if="r.plan_changed" class="flag misestimate" :title="`before: ${ids(r.baseline_plans)} · now: ${ids(r.recent_plans)}`">plan changed</span>
              <span v-else class="muted">same</span>
            </td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-else-if="!loading && !error" class="muted pad">No query got slower in this period. Try a longer period or lower the thresholds.</p>
    <p v-if="rows.length" class="muted">Together they added about {{ formatDuration(addedTotal) }} of execution time in this period.</p>
  </section>
</template>
