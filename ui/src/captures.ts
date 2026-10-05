// Display helpers for captured API requests. They only reshape the backend's
// JSON for display; they do not interpret transcripts.

import type { CaptureOverview, CaptureRecord } from './types.ts'

type Obj = Record<string, unknown>

function isObj(v: unknown): v is Obj {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

function hasCache(v: unknown): boolean {
  return isObj(v) && v.cache_control !== undefined && v.cache_control !== null
}

/** The request body as an object, or null if it is not a JSON object. */
export function requestBody(record: CaptureRecord): Record<string, unknown> | null {
  const body = record.request.body
  return body.kind === 'json' && isObj(body.value) ? body.value : null
}

/** The system prompt as text blocks (a plain string becomes one block). */
export function systemBlocks(body: Record<string, unknown> | null): { text: string; cached: boolean }[] {
  const system = body?.system
  if (typeof system === 'string') return [{ text: system, cached: false }]
  if (!Array.isArray(system)) return []
  return system.map((b) => ({
    text: isObj(b) && typeof b.text === 'string' ? b.text : JSON.stringify(b),
    cached: hasCache(b),
  }))
}

/** The tool definitions the request offered. */
export function toolList(
  body: Record<string, unknown> | null,
): { name: string; description: string; schema: unknown }[] {
  const tools = body?.tools
  if (!Array.isArray(tools)) return []
  return tools.map((t) => {
    const o = isObj(t) ? t : {}
    return {
      name: typeof o.name === 'string' ? o.name : '',
      description: typeof o.description === 'string' ? o.description : '',
      schema: o.input_schema ?? null,
    }
  })
}

function blockText(block: Obj): string {
  switch (block.type) {
    case 'text':
      return typeof block.text === 'string' ? block.text : ''
    case 'thinking':
      return typeof block.thinking === 'string' ? block.thinking : ''
    case 'tool_use': {
      const name = typeof block.name === 'string' ? block.name : ''
      return `${name} ${JSON.stringify(block.input ?? null)}`
    }
    case 'tool_result': {
      const c = block.content
      if (typeof c === 'string') return c
      if (Array.isArray(c)) {
        return c.map((x) => (isObj(x) && typeof x.text === 'string' ? x.text : JSON.stringify(x))).join('\n')
      }
      return ''
    }
    default:
      return typeof block.text === 'string' ? block.text : JSON.stringify(block)
  }
}

/** The conversation messages, each split into displayable blocks. */
export function messageList(body: Record<string, unknown> | null): {
  role: string
  blocks: { type: string; text: string; cached: boolean; raw: unknown }[]
}[] {
  const messages = body?.messages
  if (!Array.isArray(messages)) return []
  return messages.map((m) => {
    const o = isObj(m) ? m : {}
    const role = typeof o.role === 'string' ? o.role : ''
    const content = o.content
    if (typeof content === 'string') {
      return { role, blocks: [{ type: 'text', text: content, cached: false, raw: content as unknown }] }
    }
    const blocks = Array.isArray(content)
      ? content.map((b: unknown) =>
          isObj(b)
            ? {
                type: typeof b.type === 'string' ? b.type : 'unknown',
                text: blockText(b),
                cached: hasCache(b),
                raw: b as unknown,
              }
            : { type: 'unknown', text: JSON.stringify(b), cached: false, raw: b },
        )
      : []
    return { role, blocks }
  })
}

/** The capture id of the model call with this trace id, if it was captured. */
export function captureIdFor(overview: CaptureOverview | null, traceId: string): string | null {
  return overview?.calls.find((c) => c.trace_id === traceId)?.capture_id ?? null
}
