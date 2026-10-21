import { useState } from "react"

/** Copies `text` to the clipboard, and says so. */
export function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <button
      type="button"
      className="small"
      onClick={() => {
        navigator.clipboard.writeText(text).then(
          () => setCopied(true),
          () => setCopied(false),
        )
      }}
    >
      {copied ? "Copied" : "Copy"}
    </button>
  )
}
