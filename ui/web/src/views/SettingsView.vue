<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { api, session } from '../api'
import { formatDateTime } from '../lib/format'

interface Health {
  extension: { extension_version: string; api_version: number; server_version: string; connected_as: string; in_recovery: boolean } | null
  buffers: Record<string, number> | null
  statistics: Record<string, number | string | null> | null
}
interface Setting { name: string; setting: string; unit: string | null; context: string; short_desc: string }
interface Rule {
  id: number
  database: string | null
  role: string | null
  application_name: string | null
  min_duration_ms: number
  query_contains: string | null
  errors_only: boolean
}

const health = ref<Health | null>(null)
const settings = ref<Setting[]>([])
const rules = ref<Rule[]>([])
const error = ref('')
const message = ref('')
const form = reactive({ database: '', role: '', application_name: '', min_duration_ms: 0, query_contains: '', errors_only: false })

const canAdmin = computed(() => session.me?.role === 'admin' && session.me.admin_enabled)

async function load() {
  try {
    const [h, s] = await Promise.all([api.get<Health>('/api/health'), api.get<{ settings: Setting[]; rules: Rule[] }>('/api/settings')])
    health.value = h
    settings.value = s.settings
    rules.value = s.rules
    error.value = ''
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'could not load'
  }
}
onMounted(() => {
  load()
})

function pct(used: number | undefined, capacity: number | undefined): number {
  return capacity ? Math.min(100, Math.round(((used ?? 0) / capacity) * 100)) : 0
}

const rings = computed(() => {
  const b = health.value?.buffers
  if (!b) return []
  return [
    { name: 'Statements', used: b.retained, capacity: b.capacity, written: b.total_written, lost: b.overwritten },
    { name: 'Events', used: b.events_retained, capacity: b.events_capacity, written: b.events_total_written, lost: b.events_overwritten },
    { name: 'Plans', used: b.plans_retained, capacity: b.plans_capacity, written: b.plans_total_written, lost: b.plans_overwritten },
  ]
})

async function act(label: string, fn: () => Promise<unknown>) {
  message.value = ''
  try {
    await fn()
    message.value = label
    await load()
  } catch (e) {
    message.value = e instanceof Error ? e.message : 'failed'
  }
}

const addRule = () =>
  act('Capture rule added.', async () => {
    await api.post('/api/rules', { ...form })
    Object.assign(form, { database: '', role: '', application_name: '', min_duration_ms: 0, query_contains: '', errors_only: false })
  })
const removeRule = (id: number) => act('Capture rule removed.', () => api.del(`/api/rules/${id}`))
const resetBuffers = () => {
  if (window.confirm('Forget every recorded statement, event and plan on the server? This cannot be undone.')) {
    act('Buffers cleared.', () => api.post('/api/reset'))
  }
}
const describeRule = (r: Rule) =>
  [
    r.database && `database ${r.database}`,
    r.role && `role ${r.role}`,
    r.application_name && `application starts with "${r.application_name}"`,
    r.min_duration_ms > 0 && `at least ${r.min_duration_ms} ms`,
    r.query_contains && `text contains "${r.query_contains}"`,
    r.errors_only && 'errors only',
  ]
    .filter(Boolean)
    .join(' · ') || 'every statement'
</script>

<template>
  <section>
    <h1>Settings &amp; health</h1>
    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <p v-if="message" class="notice">{{ message }}</p>

    <div class="cards" v-if="health">
      <div class="card stat">
        <h3>Extension</h3>
        <p v-if="health.extension">
          pg_profiler {{ health.extension.extension_version }} · SQL API {{ health.extension.api_version }}
          <br /><small class="muted">{{ health.extension.server_version }}</small>
          <br /><small class="muted">connected as {{ health.extension.connected_as }}<span v-if="health.extension.in_recovery"> · standby</span></small>
        </p>
        <p v-else class="bad">The extension is not installed in this database.</p>
      </div>
      <div class="card stat" v-for="r in rings" :key="r.name">
        <h3>{{ r.name }} buffer</h3>
        <div class="bar" :title="`${r.used} of ${r.capacity}`"><i :style="{ width: pct(r.used, r.capacity) + '%' }"></i></div>
        <p>
          {{ r.used }} / {{ r.capacity }} ({{ pct(r.used, r.capacity) }} %)
          <br /><small class="muted">{{ r.written }} written<span v-if="r.lost"> · <span class="bad">{{ r.lost }} overwritten</span></span></small>
        </p>
      </div>
      <div class="card stat" v-if="health.statistics">
        <h3>Statistics</h3>
        <p>
          {{ health.statistics.interval_seconds }} s intervals · {{ health.statistics.intervals_in_memory }} in memory
          <br /><small class="muted">current: {{ health.statistics.current_entries }} / {{ health.statistics.entries_capacity }} entries
            · since {{ formatDateTime(String(health.statistics.current_interval ?? '')) }}</small>
          <br /><small :class="Number(health.statistics.lost_intervals) || Number(health.statistics.dropped_entries) ? 'bad' : 'muted'">
            {{ health.statistics.lost_intervals }} intervals lost · {{ health.statistics.dropped_entries }} entries dropped · {{ health.statistics.unarchived_intervals }} waiting for the archiver</small>
        </p>
      </div>
    </div>

    <h2>Capture rules</h2>
    <p class="muted">
      By default every statement is recorded. With rules, only statements that match at least one rule are kept in the
      trace (the aggregated statistics still count everything).
    </p>
    <table class="grid compact" v-if="rules.length">
      <thead><tr><th class="r">#</th><th>Keeps</th><th v-if="canAdmin"></th></tr></thead>
      <tbody>
        <tr v-for="r in rules" :key="r.id">
          <td class="r">{{ r.id }}</td>
          <td>{{ describeRule(r) }}</td>
          <td v-if="canAdmin" class="actions"><button @click="removeRule(r.id)">Remove</button></td>
        </tr>
      </tbody>
    </table>
    <p v-else class="muted">No rules: everything is recorded.</p>

    <form v-if="canAdmin" class="rule-form card" @submit.prevent="addRule">
      <h3>Add a rule</h3>
      <label>Database <input v-model="form.database" placeholder="any" /></label>
      <label>Role <input v-model="form.role" placeholder="any" /></label>
      <label>Application starts with <input v-model="form.application_name" placeholder="any" /></label>
      <label>At least (ms) <input v-model.number="form.min_duration_ms" type="number" min="0" /></label>
      <label>Text contains <input v-model="form.query_contains" placeholder="any" /></label>
      <label class="check"><input v-model="form.errors_only" type="checkbox" /> errors only</label>
      <button class="primary">Add rule</button>
    </form>
    <p v-else-if="session.me && !canAdmin" class="muted">
      Changing rules needs the admin role<span v-if="session.me.role === 'admin'"> and an admin database connection (<code>PROFILER_ADMIN_DB_URL</code>)</span>.
    </p>

    <h2 v-if="canAdmin">Maintenance</h2>
    <p v-if="canAdmin"><button class="danger" @click="resetBuffers">Clear the server's buffers…</button></p>

    <h2>Settings</h2>
    <table class="grid compact">
      <thead><tr><th>Setting</th><th>Value</th><th>Changes</th><th>Description</th></tr></thead>
      <tbody>
        <tr v-for="s in settings" :key="s.name">
          <td><code>{{ s.name }}</code></td>
          <td>{{ s.setting }}{{ s.unit ? ' ' + s.unit : '' }}</td>
          <td>{{ s.context === 'postmaster' ? 'needs a restart' : s.context === 'sighup' ? 'reload' : 'per session' }}</td>
          <td class="muted">{{ s.short_desc }}</td>
        </tr>
      </tbody>
    </table>
    <p class="muted">Settings are shown as the server reports them; change them in <code>postgresql.conf</code> or with <code>ALTER SYSTEM</code>.</p>
  </section>
</template>
