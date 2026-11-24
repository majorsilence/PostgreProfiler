// The time range shared by the Query Store screens. It survives reloads, and a
// preset range (which ends "now") moves forward every 30 seconds.
import { computed, reactive, ref } from 'vue'
import { defaultSelection, iso, resolve, type Range, type Selection } from './range'

function load(): Selection {
  try {
    const saved = JSON.parse(localStorage.getItem('store.range') ?? 'null') as Selection | null
    if (saved && typeof saved.preset === 'string') return saved
  } catch {
    // use the default
  }
  return defaultSelection()
}

export const selection = reactive<Selection>(load())

/** Bumped to make screens reload (the clock moved, or the user asked). */
export const tick = ref(0)

function save() {
  try {
    localStorage.setItem('store.range', JSON.stringify(selection))
  } catch {
    // storage may be unavailable
  }
}

export function setPreset(id: string) {
  selection.preset = id
  delete selection.from
  delete selection.to
  save()
  tick.value++
}

export function setCustom(from: Date, to: Date) {
  selection.preset = 'custom'
  selection.from = iso(from)
  selection.to = iso(to)
  save()
  tick.value++
}

/** The current window; recomputed when the selection or the tick changes. */
export const range = computed<Range>(() => {
  void tick.value
  return resolve(selection)
})

let timer: number | undefined
let users = 0

/** Keep preset ranges moving while a Query Store screen is open. */
export function startAutoRefresh(): () => void {
  users++
  if (users === 1) {
    timer = window.setInterval(() => {
      if (selection.preset !== 'custom') tick.value++
    }, 30_000)
  }
  return () => {
    users--
    if (users === 0) window.clearInterval(timer)
  }
}
