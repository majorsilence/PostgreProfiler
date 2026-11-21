// Calls to the UI server. Mutating requests carry X-Profiler-UI, which the
// server requires so that another site cannot make the browser send them.
import { reactive } from 'vue'

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message)
  }
}

export interface Me {
  user: string
  role: 'admin' | 'viewer'
  admin_enabled: boolean
}

export const session = reactive<{ me: Me | null; checked: boolean }>({ me: null, checked: false })

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json', 'X-Profiler-UI': '1' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await res.text()
  let data: unknown = null
  try {
    data = text ? JSON.parse(text) : null
  } catch {
    // not JSON (a proxy error page, say)
  }
  if (!res.ok) {
    if (res.status === 401 && path !== '/api/login') session.me = null
    const message = (data as { error?: string } | null)?.error ?? `${res.status} ${res.statusText}`
    throw new ApiError(res.status, message)
  }
  return data as T
}

export const api = {
  get: <T>(path: string) => request<T>('GET', path),
  post: <T>(path: string, body?: unknown) => request<T>('POST', path, body ?? {}),
  del: <T>(path: string) => request<T>('DELETE', path),
}

export async function loadMe(): Promise<void> {
  try {
    session.me = await api.get<Me>('/api/me')
  } catch {
    session.me = null
  }
  session.checked = true
}

export async function login(user: string, password: string): Promise<void> {
  await api.post('/api/login', { user, password })
  await loadMe()
}

export async function logout(): Promise<void> {
  try {
    await api.post('/api/logout')
  } finally {
    session.me = null
  }
}
