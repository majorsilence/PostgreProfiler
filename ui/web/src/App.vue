<script setup lang="ts">
import { watch } from 'vue'
import { RouterLink, RouterView, useRouter } from 'vue-router'
import { logout, session } from './api'
import { feedStatus } from './lib/feed'

const router = useRouter()

// A session that expired while the app was open sends us back to the login.
watch(
  () => session.me,
  (me) => {
    if (!me && session.checked && router.currentRoute.value.path !== '/login') {
      router.push({ path: '/login', query: { next: router.currentRoute.value.fullPath } })
    }
  },
)

async function signOut() {
  await logout()
  router.push('/login')
}

const feedLabel: Record<string, string> = {
  idle: 'feed idle',
  connecting: 'connecting…',
  live: 'live',
  down: 'reconnecting…',
}
</script>

<template>
  <header v-if="session.me" class="topbar">
    <div class="brand">pg_profiler</div>
    <nav>
      <RouterLink to="/trace">Live Trace</RouterLink>
      <RouterLink to="/running">Running Now</RouterLink>
      <RouterLink to="/events">Events</RouterLink>
      <RouterLink to="/settings">Settings &amp; Health</RouterLink>
    </nav>
    <div class="spacer"></div>
    <span v-if="feedStatus !== 'idle'" class="feed" :class="feedStatus" :title="'live feed: ' + feedStatus">
      <i></i>{{ feedLabel[feedStatus] }}
    </span>
    <span class="who">{{ session.me.user }} <small>({{ session.me.role }})</small></span>
    <button class="link" @click="signOut">Sign out</button>
  </header>
  <main :class="{ bare: !session.me }">
    <RouterView />
  </main>
</template>
