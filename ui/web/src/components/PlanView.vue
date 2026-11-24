<script setup lang="ts">
import { computed, ref } from 'vue'
import { parsePlan, flatten } from '../lib/plan'
import PlanNodeView from './PlanNodeView.vue'

const props = defineProps<{ text: string; note?: string }>()
const mode = ref<'tree' | 'text'>('tree')
const copied = ref(false)

const plan = computed(() => parsePlan(props.text))
const flagged = computed(() => (plan.value ? flatten(plan.value.root).flatMap((n) => n.flags.map((f) => ({ node: n.type, ...f }))) : []))

const pretty = computed(() => {
  const t = props.text.trim()
  if (t.startsWith('[') || t.startsWith('{')) {
    try {
      return JSON.stringify(JSON.parse(t), null, 2)
    } catch {
      return props.text
    }
  }
  return props.text
})

async function copy() {
  try {
    await navigator.clipboard.writeText(props.text)
    copied.value = true
    setTimeout(() => (copied.value = false), 1500)
  } catch {
    // clipboard access can be refused
  }
}
</script>

<template>
  <div class="plan card">
    <div class="toolbar">
      <div class="seg" role="tablist">
        <button role="tab" :aria-selected="mode === 'tree'" :class="{ on: mode === 'tree' }" :disabled="!plan" @click="mode = 'tree'">Tree</button>
        <button role="tab" :aria-selected="mode === 'text'" :class="{ on: mode === 'text' || !plan }" @click="mode = 'text'">Text</button>
      </div>
      <span v-if="note" class="muted">{{ note }}</span>
      <div class="spacer"></div>
      <button class="link" @click="copy">{{ copied ? 'Copied' : 'Copy plan' }}</button>
    </div>
    <template v-if="plan && mode === 'tree'">
      <ul v-if="flagged.length" class="flags-list">
        <li v-for="(f, i) in flagged" :key="i"><span class="flag" :class="f.kind">{{ f.text }}</span> {{ f.node }}</li>
      </ul>
      <div class="pn-root"><PlanNodeView :node="plan.root" :max-cost="plan.maxCost" /></div>
      <p v-if="plan.summary.length" class="muted summary">{{ plan.summary.join(' · ') }}</p>
      <p class="muted hint">Bars show each node's share of the plan's estimated cost. Click a node to see its filters and details.</p>
    </template>
    <pre v-else class="sql">{{ pretty }}</pre>
  </div>
</template>
