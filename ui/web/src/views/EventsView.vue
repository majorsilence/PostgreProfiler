<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { api } from '../api'
import { subscribe, type EventRow } from '../lib/feed'
import { formatDateTime, formatDuration } from '../lib/format'
import { loadNames, names } from '../lib/names'

const MAX = 2000
const events = ref<EventRow[]>([])
const error = ref('')
const loading = ref(true)
// Short-lived connections make logins and logouts the bulk of the events; start without them.
const hidden = ref<Set<string>>(new Set(['login', 'logout']))
const open = ref<number | null>(null)
let stop: (() => void) | null = null

const kinds = ['deadlock', 'lock_timeout', 'lock_wait', 'lock_acquired', 'cancel', 'terminated', 'auth_failure', 'login', 'logout']

onMounted(async () => {
  loadNames()
  try {
    events.value = await api.get<EventRow[]>('/api/events?limit=500')
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'could not load the events'
  } finally {
    loading.value = false
  }
  stop = subscribe((b) => {
    if (!b.events.length) return
    const top = events.value.length ? events.value[0].seq : -1
    const fresh = b.events.filter((e) => e.seq > top).reverse()
    if (fresh.length) events.value = fresh.concat(events.value).slice(0, MAX)
  })
})
onBeforeUnmount(() => stop?.())

const counts = computed(() => {
  const c = new Map<string, number>()
  for (const e of events.value) c.set(e.kind, (c.get(e.kind) ?? 0) + 1)
  return c
})
const visible = computed(() => events.value.filter((e) => !hidden.value.has(e.kind)))

function toggle(kind: string) {
  const next = new Set(hidden.value)
  if (next.has(kind)) next.delete(kind)
  else next.add(kind)
  hidden.value = next
}

const severe = new Set(['deadlock', 'lock_timeout', 'auth_failure', 'terminated'])
const who = (e: EventRow) => names.value.roles.get(String(e.userid)) ?? (Number(e.userid) ? String(e.userid) : '')
const db = (e: EventRow) => names.value.databases.get(String(e.dbid)) ?? (Number(e.dbid) ? String(e.dbid) : '')
</script>

<template>
  <section>
    <div class="toolbar">
      <h1>Events</h1>
      <div class="chips">
        <button
          v-for="k in kinds"
          :key="k"
          class="chip"
          :class="{ off: hidden.has(k), severe: severe.has(k) }"
          :aria-pressed="!hidden.has(k)"
          @click="toggle(k)"
        >
          {{ k.replace('_', ' ') }} <b>{{ counts.get(k) ?? 0 }}</b>
        </button>
      </div>
    </div>
    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <p v-if="loading" class="muted">Loading…</p>
    <p v-if="!loading && !visible.length" class="muted pad">
      No events yet. Deadlocks, cancelled statements, terminated sessions and failed logins show up here; lock waits need
      <code>log_lock_waits = on</code>.
    </p>

    <div class="grid-wrap" v-if="visible.length">
      <table class="grid">
        <thead>
          <tr>
            <th>Time</th>
            <th>Event</th>
            <th class="r">PID</th>
            <th>User</th>
            <th>Database</th>
            <th>Application</th>
            <th>Client</th>
            <th>SQLSTATE</th>
            <th class="r">Duration</th>
            <th>Detail</th>
          </tr>
        </thead>
        <tbody>
          <template v-for="e in visible.slice(0, 500)" :key="e.seq">
            <tr :class="{ severe: severe.has(e.kind) }" tabindex="0" @click="open = open === e.seq ? null : e.seq" @keydown.enter="open = open === e.seq ? null : e.seq">
              <td>{{ formatDateTime(e.ts) }}</td>
              <td><span class="kind" :class="e.kind">{{ e.kind.replace('_', ' ') }}</span></td>
              <td class="r">{{ e.pid }}</td>
              <td>{{ who(e) }}</td>
              <td>{{ db(e) }}</td>
              <td>{{ e.application_name }}</td>
              <td>{{ e.client_addr }}</td>
              <td>{{ e.sqlstate }}</td>
              <td class="r">{{ e.duration_ms ? formatDuration(e.duration_ms) : '' }}</td>
              <td class="stmt">{{ (e.detail ?? '').split('\n')[0] }}</td>
            </tr>
            <tr v-if="open === e.seq && e.detail" class="expand">
              <td colspan="10"><pre class="sql">{{ e.detail }}</pre></td>
            </tr>
          </template>
        </tbody>
      </table>
    </div>
  </section>
</template>
