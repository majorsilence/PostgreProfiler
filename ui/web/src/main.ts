import { createApp } from 'vue'
import { createRouter, createWebHistory } from 'vue-router'
import App from './App.vue'
import { loadMe, session } from './api'
import './style.css'

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', redirect: '/trace' },
    { path: '/login', component: () => import('./views/LoginView.vue'), meta: { public: true } },
    { path: '/trace', component: () => import('./views/TraceView.vue') },
    { path: '/running', component: () => import('./views/RunningView.vue') },
    { path: '/events', component: () => import('./views/EventsView.vue') },
    { path: '/store', redirect: '/store/overview' },
    { path: '/store/overview', component: () => import('./views/StoreView.vue') },
    { path: '/store/regressed', component: () => import('./views/RegressedView.vue') },
    { path: '/store/variation', component: () => import('./views/VariationView.vue') },
    { path: '/store/waits', component: () => import('./views/WaitsView.vue') },
    { path: '/store/query/:dbid/:queryid', component: () => import('./views/QueryView.vue') },
    { path: '/settings', component: () => import('./views/SettingsView.vue') },
    { path: '/:rest(.*)*', redirect: '/trace' },
  ],
})

router.beforeEach(async (to) => {
  if (!session.checked) await loadMe()
  if (!to.meta.public && !session.me) return { path: '/login', query: { next: to.fullPath } }
  if (to.path === '/login' && session.me) return { path: '/trace' }
  return true
})

createApp(App).use(router).mount('#app')
