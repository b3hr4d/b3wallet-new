import type { ReactNode } from "react"

/** The start page's shape while the wallet is read. */
export function HomeOutline({ children }: { children?: ReactNode }) {
  return (
    <div className="dashboard">
      <section className="card hero" aria-busy="true">
        <div className="hero-top">
          <span className="mark skeleton" aria-hidden="true" />
          <div className="hero-name">
            <span className="skeleton wide" />
            <span className="skeleton" />
          </div>
        </div>
        <div className="holdings">
          <span className="skeleton big" />
        </div>
        <span className="visually-hidden">Reading the wallet…</span>
      </section>
      {children}
    </div>
  )
}
