// Small display helpers. Pure functions, no DOM, so they can be unit tested
// with Node's built-in test runner.

/** `98 ms`, `4.9 s`, `1m 47s`, `1h 03m`. */
export function formatDuration(ms: number | null): string | null {
  if (ms === null || !Number.isFinite(ms) || ms < 0) return null
  if (ms < 1000) return `${Math.round(ms)} ms`
  const seconds = ms / 1000
  if (seconds < 60) return `${seconds.toFixed(1)} s`
  const totalSeconds = Math.round(seconds)
  const minutes = Math.floor(totalSeconds / 60)
  if (minutes < 60) return `${minutes}m ${String(totalSeconds % 60).padStart(2, '0')}s`
  const hours = Math.floor(minutes / 60)
  return `${hours}h ${String(minutes % 60).padStart(2, '0')}m`
}

/** `462`, `71.8k`, `1.25M`. */
export function formatTokens(tokens: number | null): string | null {
  if (tokens === null || !Number.isFinite(tokens)) return null
  if (tokens < 1000) return String(tokens)
  if (tokens < 1_000_000) return `${(tokens / 1000).toFixed(1)}k`
  return `${(tokens / 1_000_000).toFixed(2)}M`
}

/** `512 B`, `12.3 KB`, `4.5 MB`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/** `just now`, `5 min ago`, `3 h ago`, `2 days ago`, or a date when older. */
export function formatRelative(ms: number, now: number): string {
  const diff = Math.max(0, now - ms)
  const minute = 60_000
  const hour = 60 * minute
  const day = 24 * hour
  if (diff < minute) return 'just now'
  if (diff < hour) return `${Math.floor(diff / minute)} min ago`
  if (diff < day) return `${Math.floor(diff / hour)} h ago`
  if (diff < 14 * day) {
    const days = Math.floor(diff / day)
    return days === 1 ? 'yesterday' : `${days} days ago`
  }
  return new Date(ms).toISOString().slice(0, 10)
}

/** Local date and time for a timestamp, for the details panel. */
export function formatTimestamp(ms: number | null): string | null {
  if (ms === null) return null
  return new Date(ms).toLocaleString()
}

/** First 8 characters of a session id, for sessions without a title. */
export function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id
}

/** `1 tool call`, `2 tool calls`. */
export function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? '' : 's'}`
}

/** JSON with two-space indents; falls back to `String` for values JSON cannot write. */
export function prettyJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? String(value)
  } catch {
    return String(value)
  }
}
