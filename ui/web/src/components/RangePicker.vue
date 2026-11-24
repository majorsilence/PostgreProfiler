<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { describe, presets, toLocalInput } from '../lib/range'
import { range, selection, setCustom, setPreset, tick } from '../lib/rangeState'

const custom = ref(selection.preset === 'custom')
const from = ref(toLocalInput(range.value.from))
const to = ref(toLocalInput(range.value.to))

watch(
  () => selection.preset,
  (p) => {
    custom.value = p === 'custom'
    from.value = toLocalInput(range.value.from)
    to.value = toLocalInput(range.value.to)
  },
)

const value = computed({
  get: () => (selection.preset === 'custom' || custom.value ? 'custom' : selection.preset),
  set: (v: string) => {
    if (v === 'custom') {
      custom.value = true
      applyCustom()
    } else {
      custom.value = false
      setPreset(v)
    }
  },
})

const valid = computed(() => {
  const f = new Date(from.value)
  const t = new Date(to.value)
  return !Number.isNaN(f.getTime()) && !Number.isNaN(t.getTime()) && f < t
})

function applyCustom() {
  if (valid.value) setCustom(new Date(from.value), new Date(to.value))
}
</script>

<template>
  <div class="range">
    <label class="inline">
      Period
      <select v-model="value" aria-label="Time range">
        <option v-for="p in presets" :key="p.id" :value="p.id">{{ p.label }}</option>
        <option value="custom">Custom…</option>
      </select>
    </label>
    <template v-if="custom">
      <input v-model="from" type="datetime-local" aria-label="From" @change="applyCustom" />
      <span class="muted">to</span>
      <input v-model="to" type="datetime-local" aria-label="To" @change="applyCustom" />
    </template>
    <span class="muted range-text">{{ describe(range) }}</span>
    <button class="link" title="Reload" @click="tick++">↻ Refresh</button>
  </div>
</template>
