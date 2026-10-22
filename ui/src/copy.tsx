import { useState } from "react"
import { CheckIcon, CopyIcon } from "./icons.tsx"

/**
 * Copies `text` to the clipboard, and says so. With a `label` it shows only
 * its icon, and the label tells screen readers what it copies.
 */
export function CopyButton({ text, label }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      className={label ? "ghost small icon-only" : "small"}
      aria-label={label}
      title={label}
      onClick={() => {
        navigator.clipboard.writeText(text).then(
          () => setCopied(true),
          () => setCopied(false),
        )
      }}
    >
      {copied ? <CheckIcon /> : <CopyIcon />}
      {!label && (copied ? "Copied" : "Copy")}
    </button>
  )
}
