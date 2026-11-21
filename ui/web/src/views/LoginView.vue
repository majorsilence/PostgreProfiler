<script setup lang="ts">
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { login } from '../api'

const user = ref('')
const password = ref('')
const error = ref('')
const busy = ref(false)
const router = useRouter()
const route = useRoute()

async function submit() {
  error.value = ''
  busy.value = true
  try {
    await login(user.value, password.value)
    const next = typeof route.query.next === 'string' && route.query.next.startsWith('/') ? route.query.next : '/trace'
    router.push(next)
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Sign in failed'
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <form class="login-form card" @submit.prevent="submit">
    <h1>pg_profiler</h1>
    <p class="muted">Sign in to see what your PostgreSQL server is doing.</p>
    <label>
      User
      <input v-model="user" autocomplete="username" autofocus required />
    </label>
    <label>
      Password
      <input v-model="password" type="password" autocomplete="current-password" required />
    </label>
    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <button class="primary" :disabled="busy">{{ busy ? 'Signing in…' : 'Sign in' }}</button>
  </form>
</template>
