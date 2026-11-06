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
      {children}
    </div>
  )
}
