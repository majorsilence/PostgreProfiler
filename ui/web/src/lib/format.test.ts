import { describe, expect, it } from 'vitest'
import { formatBytes, formatDuration, formatMs, oneLine } from './format'

describe('format', () => {
  it('formats milliseconds with sensible precision', () => {
    expect(formatMs(0.123)).toBe('0.12')
    expect(formatMs(12.34)).toBe('12.3')
    expect(formatMs(456.7)).toBe('457')
    expect(formatMs(2500)).toBe('2.50 s')
    expect(formatMs(null)).toBe('')
  })
  it('formats bytes and durations', () => {
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(2048)).toBe('2.0 KiB')
    expect(formatDuration(250)).toBe('250 ms')
    expect(formatDuration(90000)).toBe('1 min 30 s')
  })
  it('flattens and truncates statement text', () => {
    expect(oneLine('SELECT  1\n  FROM t')).toBe('SELECT 1 FROM t')
    expect(oneLine('x'.repeat(10), 5)).toBe('xxxxx…')
  })
})
