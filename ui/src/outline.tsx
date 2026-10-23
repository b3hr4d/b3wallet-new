import type { ReactNode } from "react"

/** The start page's shape while the wallet is read. */
export function HomeOutline({ children }: { children?: ReactNode }) {
  return (
    <div className="overview">
      <section className="hero" aria-busy="true">
        <div className="hero-head">
          <span className="mark skeleton" aria-hidden="true" />
          <div className="hero-name">
            <span className="skeleton" />
            <span className="skeleton wide" />
          </div>
        </div>
        <div className="holdings">
          <div className="holding">
            <span className="skeleton" />
            <span className="skeleton big" />
          </div>
        </div>
        <span className="visually-hidden">Reading the wallet…</span>
      </section>
      <div className="instruments" aria-hidden="true">
        {[0, 1, 2, 3, 4].map((cell) => (
          <div key={cell} className="instrument">
            <span className="skeleton" />
            <span className="skeleton big" />
          </div>
        ))}
      </div>
      {children}
    </div>
  )
}

/** The rail's shape until the app runs. */
export function RailOutline() {
  return (
    <div className="rail-inner" aria-hidden="true">
      <section className="pane">
        <div className="pane-head">
          <span className="skeleton" />
        </div>
        <span className="skeleton wide" />
        <span className="skeleton wide" />
      </section>
      <section className="pane">
        <div className="pane-head">
          <span className="skeleton" />
        </div>
        {[0, 1, 2, 3].map((row) => (
          <span key={row} className="skeleton wide" />
        ))}
      </section>
    </div>
  )
}
