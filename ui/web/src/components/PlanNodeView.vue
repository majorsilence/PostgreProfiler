<script setup lang="ts">
import { computed, ref } from 'vue'
import { compact } from '../lib/chart'
import { formatMs } from '../lib/format'
import type { PlanNode } from '../lib/plan'

const props = defineProps<{ node: PlanNode; maxCost: number }>()
const open = ref(false)
const pct = computed(() => (props.maxCost > 0 ? Math.min(100, ((props.node.totalCost ?? 0) / props.maxCost) * 100) : 0))
</script>

<template>
  <div class="pn" :class="{ never: node.neverExecuted }">
    <div class="pn-head" :tabindex="node.details.length ? 0 : -1" @click="open = !open" @keydown.enter="open = !open">
      <span class="pn-toggle" aria-hidden="true">{{ node.details.length ? (open ? '▾' : '▸') : '·' }}</span>
      <span class="pn-type">{{ node.type }}</span>
      <span v-if="node.index" class="pn-rel">using {{ node.index }}</span>
      <span v-if="node.relation" class="pn-rel">on {{ node.relation }}<template v-if="node.alias"> {{ node.alias }}</template></span>
      <span class="pn-metrics">
        <span v-if="node.estRows !== undefined" title="Estimated rows per loop">est {{ compact(node.estRows) }}</span>
        <span v-if="node.actualRows !== undefined" title="Actual rows per loop">→ {{ compact(node.actualRows) }}<template v-if="(node.loops ?? 1) > 1"> ×{{ node.loops }}</template></span>
        <span v-if="node.actualMs !== undefined" title="Total time">{{ formatMs(node.actualMs) }} ms</span>
        <span v-if="node.totalCost" title="Estimated total cost">cost {{ compact(node.totalCost) }}</span>
      </span>
      <span v-for="f in node.flags" :key="f.kind + f.text" class="flag" :class="f.kind">{{ f.text }}</span>
    </div>
    <div v-if="maxCost > 0" class="pn-bar" aria-hidden="true"><i :style="{ width: pct + '%' }"></i></div>
    <ul v-if="open && node.details.length" class="pn-details">
      <li v-for="(d, i) in node.details" :key="i">{{ d }}</li>
    </ul>
    <div v-if="node.children.length" class="pn-children">
      <PlanNodeView v-for="c in node.children" :key="c.id" :node="c" :max-cost="maxCost" />
    </div>
  </div>
</template>
