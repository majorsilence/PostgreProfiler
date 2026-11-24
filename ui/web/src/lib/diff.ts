// A line diff for comparing two plans.

export interface DiffLine {
  kind: 'same' | 'add' | 'del'
  text: string
}

/** Literals in a condition line (`Index Cond: (id = 42)`), which differ between executions of one plan. */
const CONDITION = /^\s*(?:[A-Z][A-Za-z ]* )?(Cond|Filter|Key|Qual|Output):/

function maskConstants(line: string): string {
  if (!CONDITION.test(line)) return line
  return line
    .replace(/'(?:[^']|'')*'/g, '?')
    .replace(/(?<![\w.$])-?\d+(?:\.\d+)?(?![\w])/g, '?')
}

/**
 * Remove what changes from run to run (costs, rows, timings, and the constants
 * in conditions), so that two plans differ only where their *shape* differs.
 */
export function normalizePlan(text: string): string {
  return text
    .split(/\r?\n/)
    .filter((l) => !/^\s*(Planning|Execution|JIT|Total runtime)\b/.test(l))
    .map((l) =>
      l
        .replace(/\s*\(cost=[^)]*\)/g, '')
        .replace(/\s*\(actual[^)]*\)/g, '')
        .replace(/\s*\(never executed\)/g, '')
        .replace(/\b(Memory|Disk|Buckets|Batches|Heap Fetches|Rows Removed by [A-Za-z ]+):.*$/, '')
        .replace(/\s+$/, ''),
    )
    .map(maskConstants)
    .filter((l) => l.trim() !== '')
    .join('\n')
}

/** Longest-common-subsequence diff of two line lists (plans are short). */
export function diffLines(a: string[], b: string[]): DiffLine[] {
  const n = a.length
  const m = b.length
  if (n * m > 4_000_000) {
    // too large to diff line by line; show both as replaced
    return [...a.map((text) => ({ kind: 'del' as const, text })), ...b.map((text) => ({ kind: 'add' as const, text }))]
  }
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0))
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1])
    }
  }
  const out: DiffLine[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push({ kind: 'same', text: a[i] })
      i++
      j++
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.push({ kind: 'del', text: a[i++] })
    } else {
      out.push({ kind: 'add', text: b[j++] })
    }
  }
  while (i < n) out.push({ kind: 'del', text: a[i++] })
  while (j < m) out.push({ kind: 'add', text: b[j++] })
  return out
}

export function diffPlans(a: string, b: string, ignoreEstimates: boolean): DiffLine[] {
  const prep = (t: string) => (ignoreEstimates ? normalizePlan(t) : t).split(/\r?\n/)
  return diffLines(prep(a), prep(b))
}

export const changed = (d: DiffLine[]): boolean => d.some((l) => l.kind !== 'same')
