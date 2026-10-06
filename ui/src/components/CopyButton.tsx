// A button that copies text to the clipboard and says whether it worked.

import { useEffect, useRef, useState } from 'react'

import { errorMessage } from '../api.ts'

/** How long the "Copied" confirmation stays. */
const CONFIRM_MS = 1500

/**
 * `text` may be a function so large text (a whole request) is only built
 * when the button is clicked.
 */
export default function CopyButton({ text, label }: { text: string | (() => string); label: string }) {
  const [state, setState] = useState<{ kind: 'idle' } | { kind: 'copied' } | { kind: 'failed'; message: string }>({
    kind: 'idle',
  })
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(
    () => () => {
      if (timer.current !== null) clearTimeout(timer.current)
    },
    [],
  )

  const copy = () => {
    if (timer.current !== null) clearTimeout(timer.current)
    // `navigator.clipboard` is missing outside secure contexts.
    const clipboard = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
    if (!clipboard) {
      setState({ kind: 'failed', message: 'the clipboard is not available' })
      return
    }
    let value: string
    try {
      value = typeof text === 'function' ? text() : text
    } catch (err: unknown) {
      setState({ kind: 'failed', message: errorMessage(err) })
      return
    }
    clipboard
      .writeText(value)
      .then(() => {
        setState({ kind: 'copied' })
        timer.current = setTimeout(() => setState({ kind: 'idle' }), CONFIRM_MS)
      })
      .catch((err: unknown) => setState({ kind: 'failed', message: errorMessage(err) }))
  }

  return (
    <span className="copy">
      <button type="button" className="small-button" onClick={copy}>
        {label}
      </button>
      {state.kind === 'copied' && (
        <span className="copy-ok" role="status">
          Copied
        </span>
      )}
      {state.kind === 'failed' && (
        <span className="text-error small" role="alert">
          Could not copy: {state.message}
        </span>
      )}
    </span>
  )
}
