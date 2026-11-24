<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import RangePicker from '../components/RangePicker.vue'
import StoreNotice from '../components/StoreNotice.vue'
import { compact } from '../lib/chart'
import { formatMs, oneLine } from '../lib/format'
import { store, type Variation } from '../lib/qs'
import { range, startAutoRefresh } from '../lib/rangeState'
import { queryPath } from '../lib/storeCommon'

const router = useRouter()
const rows = ref<Variation[]>([])
const error = ref('')
const loading = ref(true)
const minCalls = ref(10)
const stop = startAutoRefresh()
onBeforeUnmount(stop)

let seq = 0
async function load() {
  const mine = ++seq
  loading.value = true
  try {
    const r = await store.variation(range.value, minCalls.value)
    if (mine !== seq) return
    rows.value = r
    error.value = ''
  } catch (e) {
    if (mine === seq) error.value = e instanceof Error ? e.message : 'could not load'
  } finally {
    if (mine === seq) loading.value = false
  }
}
watch([range, minCalls], load, { immediate: true })

const maxCv = computed(() => Math.max(0.0001, ...rows.value.map((r) => r.cv ?? 0)))
</script>

<template>
  <section>
    <div class="toolbar">
      <h1>High variation</h1>
      <RangePicker />
    </div>
    <StoreNotice />
    <p class="muted">
      Queries whose run time is unpredictable: the standard deviation relative to the average (the coefficient of
      variation), highest first. Percentiles come from a latency histogram and are accurate to about 20 %.
    </p>
    <div class="filters">
      <label class="inline">At least <input v-model.number="minCalls" type="number" min="1" class="narrow" /> executions</label>
      <span v-if="loading" class="muted">Loading…</span>
    </div>
    <p v-if="error" class="error" role="alert">{{ error }}</p>

    <div class="grid-wrap" v-if="rows.length">
      <table class="grid">
        <thead>
          <tr>
            <th>Statement</th>
            <th class="r">Executions</th>
            <th class="r">Average (ms)</th>
            <th class="r">Std dev (ms)</th>
            <th>Variation</th>
            <th class="r">Min</th>
            <th class="r">p50</th>
            <th class="r">p95</th>
            <th class="r">p99</th>
            <th class="r">Max</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in rows" :key="r.dbid + ':' + r.queryid" tabindex="0" @click="router.push(queryPath(r.dbid, r.queryid))" @keydown.enter="router.push(queryPath(r.dbid, r.queryid))">
            <td class="stmt" :title="r.query ?? ''">{{ oneLine(r.query, 120) || '(text not available)' }}</td>
            <td class="r">{{ compact(r.calls) }}</td>
            <td class="r">{{ formatMs(r.avg_exec_ms) }}</td>
            <td class="r">{{ formatMs(r.stddev_exec_ms) }}</td>
            <td class="cv"><div class="bar"><i :style="{ width: ((r.cv ?? 0) / maxCv) * 100 + '%' }"></i></div>{{ r.cv === null ? '' : r.cv.toFixed(2) }}</td>
            <td class="r">{{ formatMs(r.min_exec_ms) }}</td>
            <td class="r">{{ formatMs(r.p50_exec_ms) }}</td>
            <td class="r">{{ formatMs(r.p95_exec_ms) }}</td>
            <td class="r">{{ formatMs(r.p99_exec_ms) }}</td>
            <td class="r">{{ formatMs(r.max_exec_ms) }}</td>
          </tr>
        </tbody>
      </table>
    </div>
    <p v-else-if="!loading && !error" class="muted pad">No query has enough executions in this period.</p>
  </section>
</template>
