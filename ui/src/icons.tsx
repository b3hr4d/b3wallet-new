import type { ReactNode } from "react"
import { dotsOf, hueClass } from "./mark.ts"

/**
 * The pages' icons, drawn inline: an image would be one more file in the
 * release, and an icon font one more origin for the CSP to allow. Each strokes
 * in the text's colour and is hidden from screen readers, so the text beside
 * it says what it means.
 */
function Icon({ children }: { children: ReactNode }) {
  return (
    <svg
      className="icon"
      viewBox="0 0 24 24"
      width="18"
      height="18"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  )
}

export const OverviewIcon = () => (
  <Icon>
    <rect x="4" y="4" width="7" height="7" rx="2" />
    <rect x="13" y="4" width="7" height="7" rx="2" />
    <rect x="4" y="13" width="7" height="7" rx="2" />
    <rect x="13" y="13" width="7" height="7" rx="2" />
  </Icon>
)

export const SendIcon = () => (
  <Icon>
    <path d="M7 17 17 7M9 7h8v8" />
  </Icon>
)

export const ReceiveIcon = () => (
  <Icon>
    <path d="M17 7 7 17M15 17H7V9" />
  </Icon>
)

export const GearIcon = () => (
  <Icon>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 13.5a7.6 7.6 0 0 0 0-3l2-1.6-2-3.4-2.4 1a7.4 7.4 0 0 0-2.6-1.5L14 2.5h-4l-.4 2.5A7.4 7.4 0 0 0 7 6.5l-2.4-1-2 3.4 2 1.6a7.6 7.6 0 0 0 0 3l-2 1.6 2 3.4 2.4-1a7.4 7.4 0 0 0 2.6 1.5l.4 2.5h4l.4-2.5a7.4 7.4 0 0 0 2.6-1.5l2.4 1 2-3.4-2-1.6Z" />
  </Icon>
)

export const ApproveIcon = () => (
  <Icon>
    <circle cx="12" cy="12" r="8.5" />
    <path d="m8.5 12.2 2.4 2.4 4.6-4.9" />
  </Icon>
)

export const CopyIcon = () => (
  <Icon>
    <rect x="9" y="9" width="11" height="11" rx="2.5" />
    <path d="M15 5.5V5a1 1 0 0 0-1-1H6a2 2 0 0 0-2 2v8a1 1 0 0 0 1 1h.5" />
  </Icon>
)

export const CheckIcon = () => (
  <Icon>
    <path d="m5 12.5 4.5 4.5L19 7.5" />
  </Icon>
)

export const RefreshIcon = () => (
  <Icon>
    <path d="M19.5 10A7.5 7.5 0 0 0 6 6.6L4.5 8M4.5 4v4h4" />
    <path d="M4.5 14A7.5 7.5 0 0 0 18 17.4l1.5-1.4M19.5 20v-4h-4" />
  </Icon>
)

export const ChevronIcon = () => (
  <Icon>
    <path d="m6.5 9.5 5.5 5.5 5.5-5.5" />
  </Icon>
)

export const ArrowIcon = () => (
  <Icon>
    <path d="M5 12h14M13 6l6 6-6 6" />
  </Icon>
)

export const PlusIcon = () => (
  <Icon>
    <path d="M12 5v14M5 12h14" />
  </Icon>
)

export const ShieldIcon = () => (
  <Icon>
    <path d="M12 3.5 19 6v5.5c0 4.2-2.8 7.4-7 9-4.2-1.6-7-4.8-7-9V6l7-2.5Z" />
    <path d="m9.2 12 2 2 3.8-4" />
  </Icon>
)

export const PeopleIcon = () => (
  <Icon>
    <circle cx="9" cy="8.5" r="3.5" />
    <path d="M3 19.5c.7-3 3-5 6-5s5.3 2 6 5" />
    <path d="M15.5 5.3a3.5 3.5 0 0 1 0 6.4M17.5 14.8c1.6.6 2.9 2.2 3.4 4.7" />
  </Icon>
)

export const PulseIcon = () => (
  <Icon>
    <path d="M3 12h4l2.5-6.5 5 13L17 12h4" />
  </Icon>
)

export const BoltIcon = () => (
  <Icon>
    <path d="M13 3 5.5 13.5H11L10 21l7.5-10.5H12L13 3Z" />
  </Icon>
)

export const LayersIcon = () => (
  <Icon>
    <path d="m12 3.5 8.5 4.5-8.5 4.5L3.5 8 12 3.5Z" />
    <path d="m3.5 12 8.5 4.5 8.5-4.5M3.5 16l8.5 4.5 8.5-4.5" />
  </Icon>
)

export const AlertIcon = () => (
  <Icon>
    <path d="M10.3 4.6 3 17.5A2 2 0 0 0 4.7 20.5h14.6a2 2 0 0 0 1.7-3L13.7 4.6a2 2 0 0 0-3.4 0Z" />
    <path d="M12 9.5v4M12 17h.01" />
  </Icon>
)

export const KeyIcon = () => (
  <Icon>
    <circle cx="8" cy="15.5" r="4" />
    <path d="m11 12.5 8.5-8.5M16.5 7l2.5 2.5M14 9.5l2 2" />
  </Icon>
)

export const CrossIcon = () => (
  <Icon>
    <path d="M7 7l10 10M17 7 7 17" />
  </Icon>
)

export const DotIcon = () => (
  <Icon>
    <circle cx="12" cy="12" r="2.5" fill="currentColor" stroke="none" />
  </Icon>
)

/**
 * The mark of the wallet with canister id `id`: its hue, and its pattern of
 * dots (see `mark.ts`).
 */
export function WalletMark({ id }: { id: string }) {
  return (
    <span className={`mark ${hueClass(id)}`} aria-hidden="true">
      <svg viewBox="0 0 5 5" width="30" height="30">
        {dotsOf(id).map(([column, row]) => (
          <circle
            key={`${column} ${row}`}
            cx={column + 0.5}
            cy={row + 0.5}
            r="0.34"
            fill="#fff"
            fillOpacity={(column + row) % 2 === 0 ? 1 : 0.62}
          />
        ))}
      </svg>
    </span>
  )
}
