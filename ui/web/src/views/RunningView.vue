<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { api, session } from '../api'
import { blockingTree, type RunningRow } from '../lib/blocking'
import { formatDuration, oneLine } from '../lib/format'

const rows = ref<RunningRow[]>([])
const error = ref('')
const message = ref('')
const loaded = ref(false)
const tree = ref(true)
const expanded = ref<number | null>(null)
let timer: number | undefined

async function refresh() {
  try {
    rows.value = await api.get<RunningRow[]>('/api/running')
    error.value = ''
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'could not load'
  } finally {
    loaded.value = true
  }
}

onMounted(() => {
  refresh()
  timer = window.setInterval(refresh, 2000)
})
onBeforeUnmount(() => window.clearInterval(timer))

const lines = computed(() =>
  tree.value ? blockingTree(rows.value) : rows.value.map((row) => ({ pid: row.pid, depth: 0, row, alsoBlockedBy: row.blocked_by?.slice(1) ?? [] })),
)
const blockedCount = computed(() => rows.value.filter((r) => (r.blocked_by?.length ?? 0) > 0).length)
const canAdmin = computed(() => session.me?.role === 'admin' && session.me.admin_enabled)

async function signal(pid: number, action: 'cancel' | 'terminate') {
  const what = action === 'cancel' ? 'Cancel the current statement of' : 'Terminate the connection of'
  if (!window.confirm(`${what} backend ${pid}?`)) return
  message.value = ''
  try {
    const r = await api.post<{ done: boolean }>(`/api/backends/${pid}/${action}`)
    message.value = r.done ? `Backend ${pid}: ${action} sent.` : `Backend ${pid}: nothing was done (it may have finished).`
    refresh()
  } catch (e) {
    message.value = e instanceof Error ? e.message : 'failed'
  }
}

function wait(r: RunningRow): string {
  return r.wait_event ? `${r.wait_event_type ?? ''}: ${r.wait_event}` : ''
}
</script>

<template>
  <section>
    <div class="toolbar">
      <h1>Running now</h1>
      <label class="check"><input v-model="tree" type="checkbox" /> Show blocking tree</label>
      <div class="spacer"></div>
      <span class="muted">{{ rows.length }} running · {{ blockedCount }} blocked · refreshes every 2 s</span>
    </div>
    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <p v-if="message" class="notice">{{ message }}</p>
    <p v-if="loaded && !rows.length && !error" class="muted pad">No statement is running right now.</p>

    <div class="grid-wrap" v-if="rows.length">
      <table class="grid">
        <thead>
          <tr>
            <th class="r">PID</th>
            <th>User</th>
            <th>Database</th>
            <th>Application</th>
            <th>State</th>
            <th>Waiting for</th>
            <th class="r">Elapsed</th>
            <th>Blocked by</th>
            <th>Statement</th>
            <th v-if="canAdmin"></th>
          </tr>
        </thead>
        <tbody>
          <template v-for="l in lines" :key="l.pid">
            <tr v-if="l.row" :class="{ blocked: (l.row.blocked_by?.length ?? 0) > 0, slow: (l.row.elapsed_ms ?? 0) > 5000 }" @click="expanded = expanded === l.pid ? null : l.pid">
              <td class="r"><span class="indent" :style="{ width: l.depth * 16 + 'px' }"></span><span v-if="l.depth" class="arrow">↳</span>{{ l.pid }}</td>
              <td>{{ l.row.usename }}</td>
              <td>{{ l.row.datname }}</td>
              <td>{{ l.row.application_name }}</td>
              <td>{{ l.row.state }}</td>
              <td>{{ wait(l.row) }}</td>
              <td class="r">{{ formatDuration(l.row.elapsed_ms) }}</td>
              <td>
                {{ l.row.blocked_by?.join(', ') }}
              </td>
              <td class="stmt" :title="l.row.query ?? ''">{{ oneLine(l.row.query, 160) }}</td>
              <td v-if="canAdmin" class="actions">
                <button @click.stop="signal(l.pid, 'cancel')" title="pg_cancel_backend">Cancel</button>
                <button class="danger" @click.stop="signal(l.pid, 'terminate')" title="pg_terminate_backend">Terminate</button>
              </td>
            </tr>
            <tr v-else class="placeholder">
              <td class="r"><span class="indent" :style="{ width: l.depth * 16 + 'px' }"></span>{{ l.pid }}</td>
              <td colspan="8" class="muted">Not running a statement (for example idle in a transaction), and holding locks others wait for.</td>
              <td v-if="canAdmin" class="actions">
                <button class="danger" @click.stop="signal(l.pid, 'terminate')">Terminate</button>
              </td>
            </tr>
            <tr v-if="l.row && expanded === l.pid" class="expand">
              <td :colspan="canAdmin ? 10 : 9"><pre class="sql">{{ l.row.query }}</pre></td>
            </tr>
          </template>
        </tbody>
      </table>
    </div>
  </section>
</template>
