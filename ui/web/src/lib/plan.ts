// Parse EXPLAIN output (text or JSON) into a tree, and flag what is worth a look.

export interface PlanNode {
  id: number
  type: string
  relation?: string
  alias?: string
  index?: string
  startupCost?: number
  totalCost?: number
  estRows?: number
  width?: number
  /** Rows per loop, when the plan has actual numbers. */
  actualRows?: number
  loops?: number
  /** Total time in ms (per loop times loops), when the plan has timing. */
  actualMs?: number
  neverExecuted?: boolean
  /** Property lines such as `Filter: (a > 1)` or `Sort Method: quicksort  Memory: 25kB`. */
  details: string[]
  children: PlanNode[]
  flags: Flag[]
}

export interface Flag {
  kind: 'misestimate' | 'spill' | 'seqscan' | 'never'
  text: string
}

export interface Plan {
  root: PlanNode
  /** Lines such as `Planning Time: 0.1 ms`. */
  summary: string[]
  hasActuals: boolean
  maxCost: number
}

const COST = /\(cost=([\d.]+)\.\.([\d.]+) rows=(\d+) width=(\d+)\)/
const ACTUAL = /\(actual(?: time=([\d.]+)\.\.([\d.]+))? rows=([\d.]+) loops=(\d+)\)/
const NEVER = /\(never executed\)/
const SUMMARY = /^(Planning|Execution|JIT|Settings|Trigger|Total runtime|Query Identifier)\b/

let nextId = 1

function newNode(type: string): PlanNode {
  return { id: nextId++, type, details: [], children: [], flags: [] }
}

/** Fill in the fields of a node from its header line (text format). */
function parseHeader(text: string): PlanNode {
  const costAt = text.search(/\s+\(cost=/)
  const never = NEVER.test(text)
  let title = costAt >= 0 ? text.slice(0, costAt) : text.replace(ACTUAL, '').replace(NEVER, '')
  title = title.trim()
  const node = newNode(title)
  const using = title.match(/^(.*?)\s+using\s+(\S+)(.*)$/)
  let rest = title
  if (using) {
    node.index = using[2]
    rest = `${using[1]}${using[3]}`
  }
  const on = rest.match(/^(.*?)\s+on\s+(\S+)(?:\s+(\S+))?\s*$/)
  if (on) {
    node.type = on[1].trim()
    node.relation = on[2]
    if (on[3] && on[3] !== on[2]) node.alias = on[3]
  } else {
    node.type = rest.trim()
  }
  const c = text.match(COST)
  if (c) {
    node.startupCost = Number(c[1])
    node.totalCost = Number(c[2])
    node.estRows = Number(c[3])
    node.width = Number(c[4])
  }
  const a = text.match(ACTUAL)
  if (a) {
    node.actualRows = Number(a[3])
    node.loops = Number(a[4])
    if (a[2] !== undefined) node.actualMs = Number(a[2]) * Number(a[4])
  }
  if (never) node.neverExecuted = true
  return node
}

function parseText(text: string): Plan | null {
  const lines = text.split(/\r?\n/).filter((l) => l.trim() !== '')
  if (lines.length === 0) return null
  let root: PlanNode | null = null
  const stack: { col: number; node: PlanNode }[] = []
  const summary: string[] = []

  for (const raw of lines) {
    const indent = raw.length - raw.trimStart().length
    const trimmed = raw.trim()
    if (indent === 0 && root && SUMMARY.test(trimmed)) {
      summary.push(trimmed)
      continue
    }
    if (trimmed.startsWith('->')) {
      const node = parseHeader(trimmed.slice(2).trim())
      while (stack.length && stack[stack.length - 1].col >= indent) stack.pop()
      const parent = stack[stack.length - 1]?.node
      if (parent) parent.children.push(node)
      else if (!root) root = node
      stack.push({ col: indent, node })
    } else if (!root) {
      root = parseHeader(trimmed)
      stack.push({ col: -1, node: root })
    } else {
      // A property line belongs to the deepest node that starts left of it,
      // or is a header such as "InitPlan 1" or "SubPlan 2" that groups children.
      while (stack.length > 1 && stack[stack.length - 1].col >= indent) stack.pop()
      const owner = stack[stack.length - 1]?.node ?? root
      if (/^(InitPlan|SubPlan|CTE)\b/.test(trimmed) && !trimmed.includes(':')) {
        const group = newNode(trimmed)
        owner.children.push(group)
        stack.push({ col: indent, node: group })
      } else {
        owner.details.push(trimmed)
      }
    }
  }
  return root ? finish(root, summary) : null
}

// ---- JSON

type J = Record<string, unknown>

function fromJson(p: J): PlanNode {
  const node = newNode(String(p['Node Type'] ?? 'Node'))
  if (p['Relation Name']) node.relation = String(p['Relation Name'])
  if (p['Alias'] && p['Alias'] !== p['Relation Name']) node.alias = String(p['Alias'])
  if (p['Index Name']) node.index = String(p['Index Name'])
  if (typeof p['Startup Cost'] === 'number') node.startupCost = p['Startup Cost']
  if (typeof p['Total Cost'] === 'number') node.totalCost = p['Total Cost']
  if (typeof p['Plan Rows'] === 'number') node.estRows = p['Plan Rows']
  if (typeof p['Plan Width'] === 'number') node.width = p['Plan Width']
  if (typeof p['Actual Rows'] === 'number') node.actualRows = p['Actual Rows']
  if (typeof p['Actual Loops'] === 'number') {
    node.loops = p['Actual Loops']
    if (node.loops === 0) node.neverExecuted = true
    if (typeof p['Actual Total Time'] === 'number') node.actualMs = p['Actual Total Time'] * node.loops
  }
  const skip = new Set(['Node Type', 'Relation Name', 'Alias', 'Index Name', 'Startup Cost', 'Total Cost', 'Plan Rows', 'Plan Width',
    'Actual Rows', 'Actual Loops', 'Actual Total Time', 'Actual Startup Time', 'Plans', 'Parallel Aware', 'Async Capable',
    'Parent Relationship', 'Subplan Name', 'Schema', 'Output', 'Plan ID', 'Disabled'])
  for (const [k, v] of Object.entries(p)) {
    if (skip.has(k) || v === null || typeof v === 'object') continue
    node.details.push(`${k}: ${String(v)}`)
  }
  if (p['Subplan Name']) node.type = `${String(p['Subplan Name'])}: ${node.type}`
  for (const c of (p['Plans'] as J[] | undefined) ?? []) node.children.push(fromJson(c))
  return node
}

function parseJson(text: string): Plan | null {
  let data: unknown
  try {
    data = JSON.parse(text)
  } catch {
    return null
  }
  const top = (Array.isArray(data) ? data[0] : data) as J | undefined
  const plan = top?.['Plan'] as J | undefined
  if (!plan) return null
  const summary: string[] = []
  for (const k of ['Planning Time', 'Execution Time']) if (typeof top?.[k] === 'number') summary.push(`${k}: ${top[k]} ms`)
  return finish(fromJson(plan), summary)
}

// ---- common

function walk(n: PlanNode, f: (n: PlanNode) => void) {
  f(n)
  n.children.forEach((c) => walk(c, f))
}

/** Add the flags and compute the plan-wide numbers. */
function finish(root: PlanNode, summary: string[]): Plan {
  let hasActuals = false
  let maxCost = 0
  walk(root, (n) => {
    if (n.actualRows !== undefined || n.neverExecuted) hasActuals = true
    maxCost = Math.max(maxCost, n.totalCost ?? 0)
    n.flags = flagsOf(n)
  })
  return { root, summary, hasActuals, maxCost }
}

/** Largest factor between the estimate and the actual rows per loop; 0 when it cannot be told. */
export function misestimate(n: PlanNode): number {
  if (n.estRows === undefined || n.actualRows === undefined) return 0
  const est = n.estRows
  const act = n.actualRows
  if (Math.max(est, act) < 100) return 0
  const [e, a] = [Math.max(est, 1), Math.max(act, 1)]
  return Math.max(e / a, a / e)
}

function flagsOf(n: PlanNode): Flag[] {
  const flags: Flag[] = []
  if (n.neverExecuted) flags.push({ kind: 'never', text: 'never executed' })
  const m = misestimate(n)
  if (m >= 10) {
    flags.push({
      kind: 'misestimate',
      text: `${n.actualRows! > n.estRows! ? 'under' : 'over'}estimated ${m >= 100 ? Math.round(m) : m.toFixed(1)}×`,
    })
  }
  const text = n.details.join('\n')
  const disk = text.match(/Disk:\s*(\d+\s*\w*)/)
  const batches = text.match(/Batches:\s*(\d+)/)
  if (disk) flags.push({ kind: 'spill', text: `spilled to disk (${disk[1]})` })
  else if (batches && Number(batches[1]) > 1) flags.push({ kind: 'spill', text: `${batches[1]} batches (hash spilled)` })
  if (n.type === 'Seq Scan' && (n.estRows ?? 0) >= 10_000) flags.push({ kind: 'seqscan', text: `sequential scan of ~${Math.round(n.estRows!)} rows` })
  return flags
}

/** Parse EXPLAIN output in either format. Null if it is neither. */
export function parsePlan(text: string): Plan | null {
  const t = text.trim()
  if (!t) return null
  nextId = 1
  return t.startsWith('[') || t.startsWith('{') ? parseJson(t) : parseText(t)
}

export function nodeLabel(n: PlanNode): string {
  let s = n.type
  if (n.index) s += ` using ${n.index}`
  if (n.relation) s += ` on ${n.relation}${n.alias ? ` ${n.alias}` : ''}`
  return s
}

/** Every node, parents first. */
export function flatten(root: PlanNode): PlanNode[] {
  const out: PlanNode[] = []
  walk(root, (n) => out.push(n))
  return out
}
