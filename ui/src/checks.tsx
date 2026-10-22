import type { Check } from "./health.ts"

/**
 * Checks as a list, each a shape and a word: the logo's node while ok, a
 * ringed node when it needs attention, a danger diamond for a problem. The
 * health panel, the settings checkup and the notifications page draw them so.
 */
export function CheckList({ checks }: { checks: readonly Check[] }) {
  return (
    <ul className="health-list">
      {checks.map((check) => (
        <li key={check.title} className="health-item">
          <span className={`health-mark ${check.level}`} aria-hidden="true" />
          <div>
            <p className="health-title">
              {check.href ? <a href={check.href}>{check.title}</a> : check.title}
              {check.level !== "ok" && (
                <span className="visually-hidden">
                  {check.level === "problem" ? " (a problem)" : " (needs attention)"}
                </span>
              )}
            </p>
            <p className="health-text">{check.text}</p>
          </div>
        </li>
      ))}
    </ul>
  )
}
