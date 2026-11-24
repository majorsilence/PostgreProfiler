<script setup lang="ts">
import { onMounted } from 'vue'
import { loadStatus, status } from '../lib/storeCommon'

onMounted(loadStatus)
</script>

<template>
  <p v-if="status && !status.has_history" class="notice">
    There is no stored history in the database this UI is connected to
    (<code>{{ status.database }}</code>). Statistics still in memory are shown
    ({{ status.memory_entries }} entries), but history needs the connection to point at the archive database:
    the one named by <code>pg_profiler.archive_database</code>, which must have <code>CREATE EXTENSION pg_profiler</code>.
    Intervals are archived once they have closed (every <code>pg_profiler.stats_interval</code> seconds).
  </p>
</template>
