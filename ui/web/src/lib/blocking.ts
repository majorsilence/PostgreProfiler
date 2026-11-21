// Turn the running statements and their `blocked_by` lists into a tree.

export interface RunningRow {
  pid: number
  usename: string | null
  datname: string | null
  application_name: string | null
  client_addr: string | null
  state: string | null
  wait_event_type: string | null
  wait_event: string | null
  queryid: string | number
  cmd_type: string
  start_time: string | null
  xact_start: string | null
  elapsed_ms: number | null
  blocked_by: number[] | null
  query: string | null
}

export interface TreeRow {
  pid: number
  depth: number
  /** The row, or null for a blocker that is not running a statement (for example idle in transaction). */
  row: RunningRow | null
  /** Other sessions that also block this one. */
  alsoBlockedBy: number[]
}

/**
 * Order the rows so that a blocked session follows the session that blocks it,
 * indented one level deeper. A session blocked by several is shown under the
 * first one; the others are listed in `alsoBlockedBy`. Blockers that do not
 * appear in `rows` become placeholder roots, because a session idle in a
 * transaction is the most common blocker and has no running statement.
 */
export function blockingTree(rows: RunningRow[]): TreeRow[] {
  const byPid = new Map(rows.map((r) => [r.pid, r]))
  const children = new Map<number, RunningRow[]>()
  const placeholders = new Set<number>()
  const roots: RunningRow[] = []

  for (const r of rows) {
    const blockers = r.blocked_by ?? []
    if (blockers.length === 0) {
      roots.push(r)
      continue
    }
    const first = blockers[0]
    if (!byPid.has(first)) placeholders.add(first)
    const list = children.get(first) ?? []
    list.push(r)
    children.set(first, list)
  }

  const out: TreeRow[] = []
  const seen = new Set<number>()
  const visit = (pid: number, row: RunningRow | null, depth: number) => {
    if (seen.has(pid)) return // a deadlock cycle would otherwise never end
    seen.add(pid)
    out.push({ pid, depth, row, alsoBlockedBy: (row?.blocked_by ?? []).slice(1) })
    for (const c of children.get(pid) ?? []) visit(c.pid, c, depth + 1)
  }

  const rootPids = new Set<number>()
  for (const r of roots) {
    rootPids.add(r.pid)
    visit(r.pid, r, 0)
  }
  for (const pid of placeholders) {
    if (!rootPids.has(pid) && !byPid.has(pid)) visit(pid, null, 0)
  }
  // Rows that are only part of a cycle have no root; show them anyway.
  for (const r of rows) if (!seen.has(r.pid)) visit(r.pid, r, 0)
  return out
}
