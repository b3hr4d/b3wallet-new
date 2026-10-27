import { useState } from "react"
import { CheckIcon, CopyIcon } from "./icons.tsx"

/**
 * Copies `text` to the clipboard, and says so. With a `label` it shows only
 * its icon, and the label tells screen readers what it copies; with a `name`
 * it is a full button of that name, the primary one with `primary`.
 */
export function CopyButton({
  text,
  label,
  name,
  primary = false,
}: {
  text: string
  label?: string
  name?: string
  primary?: boolean
}) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      className={label ? "ghost small icon-only" : name ? (primary ? "primary" : "") : "small"}
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
      {!label && (copied ? "Copied" : (name ?? "Copy"))}
    </button>
  )
}
