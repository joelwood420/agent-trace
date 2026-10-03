import assert from 'node:assert/strict'
import { test } from 'node:test'

import { formatBytes, formatDuration, formatRelative, formatTokens, plural, shortId } from './format.ts'

test('durations', () => {
  assert.equal(formatDuration(null), null)
  assert.equal(formatDuration(98), '98 ms')
  assert.equal(formatDuration(4900), '4.9 s')
  assert.equal(formatDuration(107_000), '1m 47s')
  assert.equal(formatDuration(3_780_000), '1h 03m')
})

test('tokens', () => {
  assert.equal(formatTokens(null), null)
  assert.equal(formatTokens(462), '462')
  assert.equal(formatTokens(71_808), '71.8k')
  assert.equal(formatTokens(1_250_000), '1.25M')
})

test('bytes', () => {
  assert.equal(formatBytes(512), '512 B')
  assert.equal(formatBytes(131_777), '128.7 KB')
  assert.equal(formatBytes(5 * 1024 * 1024), '5.0 MB')
})

test('relative times', () => {
  const now = Date.UTC(2026, 0, 10)
  assert.equal(formatRelative(now - 5_000, now), 'just now')
  assert.equal(formatRelative(now - 5 * 60_000, now), '5 min ago')
  assert.equal(formatRelative(now - 3 * 3_600_000, now), '3 h ago')
  assert.equal(formatRelative(now - 24 * 3_600_000, now), 'yesterday')
  assert.equal(formatRelative(now - 3 * 24 * 3_600_000, now), '3 days ago')
  assert.equal(formatRelative(Date.UTC(2025, 5, 1), now), '2025-06-01')
})

test('short ids and plurals', () => {
  assert.equal(shortId('00000000-0000-4000-8000-000000000002'), '00000000')
  assert.equal(shortId('abc'), 'abc')
  assert.equal(plural(1, 'tool call'), '1 tool call')
  assert.equal(plural(2, 'tool call'), '2 tool calls')
})
