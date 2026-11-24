<script setup lang="ts">
import { computed, ref } from 'vue'
import { changed, diffPlans } from '../lib/diff'

const props = defineProps<{ a: string; b: string; labelA: string; labelB: string }>()
const ignoreEstimates = ref(true)
const lines = computed(() => diffPlans(props.a, props.b, ignoreEstimates.value))
const different = computed(() => changed(lines.value))
</script>

<template>
  <div class="plan card">
    <div class="toolbar">
      <b>Comparing</b>
      <span class="del-tag">− {{ labelA }}</span>
      <span class="add-tag">+ {{ labelB }}</span>
      <div class="spacer"></div>
      <label class="check"><input v-model="ignoreEstimates" type="checkbox" /> ignore costs, row counts, timings and constants</label>
    </div>
    <p v-if="!different" class="muted">The two plans have the same shape<span v-if="ignoreEstimates"> (only costs, estimates or timings differ, if at all)</span>.</p>
    <pre class="diff"><span v-for="(l, i) in lines" :key="i" class="line" :class="l.kind">{{ l.kind === 'add' ? '+ ' : l.kind === 'del' ? '- ' : '  ' }}{{ l.text }}
</span></pre>
  </div>
</template>
