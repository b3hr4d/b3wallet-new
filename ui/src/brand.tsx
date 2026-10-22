/**
 * B3Wallet's logo, as B3Pay's design system locks it (`logo-v1.1`, in
 * B3Pay/b3pay-design-system, `project/assets/Logos/b3wallet-*.svg`): the
 * stem is one block, and the amber node rests in the 3's lower bowl. The
 * mark's geometry here is those files' own, unchanged; only its colours come
 * from app.css, ink for the stem, the 3 and the name and accent for the node,
 * so one drawing is both the dark and the light file. Change the mark in the
 * design system, never here.
 */

/** The 3, for Bitcoin, Ethereum and ICP, in the mark's 96 by 96 grid. */
export const THREE =
  "M65.25 62.5C65.25 59.7152 64.1437 57.0445 62.1745 55.0755C60.2055 53.1063 57.5348 52 54.75 52H39.75C38.0932 52 36.75 50.6568 36.75 49V43C36.75 41.3432 38.0932 40 39.75 40H52.75C55.0042 40 57.1662 39.1045 58.7605 37.5105C60.3545 35.9162 61.25 33.7542 61.25 31.5C61.25 29.2458 60.3545 27.0837 58.7605 25.4895C57.1662 23.8955 55.0042 23 52.75 23H39.75C38.0932 23 36.75 21.6568 36.75 20V14C36.75 12.3431 38.0932 11 39.75 11H52.75C58.187 11 63.4012 13.1598 67.2458 17.0043C71.0902 20.8488 73.25 26.063 73.25 31.5C73.25 36.3135 71.5563 40.9735 68.4652 44.6635C71.1983 46.765 73.4115 49.4663 74.9347 52.5588C76.4578 55.6515 77.25 59.0528 77.25 62.5C77.25 68.4673 74.8795 74.1903 70.66 78.41C66.4403 82.6295 60.7172 85 54.75 85H39.75C38.0932 85 36.75 83.6568 36.75 82V76C36.75 74.3432 38.0932 73 39.75 73H54.75C57.5348 73 60.2055 71.8937 62.1745 69.9245C64.1437 67.9555 65.25 65.2848 65.25 62.5Z"

/**
 * "Wallet" in Rubik 500, outlined as the design system's
 * `tools/logo/wordmark-outline.py` outlines the wordmark: font units, at
 * -0.03em letter-spacing, with the font's own kerning. At the mark's height,
 * 500's stem is the mark's: 131 font units to the block's 132.
 */
const WALLET =
  "M193 0Q178 0 169.5 -8.5Q161 -17 158 -30L39 -671Q39 -674 38.5 -676Q38 -678 38 -679Q38 -688 44.5 -694Q51 -700 59 -700L147 -700Q172 -700 175 -680L258 -227L348 -520Q351 -528 358.5 -536.5Q366 -545 381 -545L435 -545Q451 -545 458 -536.5Q465 -528 467 -520L558 -228L641 -680Q644 -700 669 -700L757 -700Q765 -700 771 -694Q777 -688 777 -679Q777 -678 777 -676Q777 -674 776 -671L657 -30Q655 -17 646 -8.5Q637 0 622 0L555 0Q540 0 531.5 -8Q523 -16 520 -25L408 -359L296 -25Q292 -16 284 -8Q276 0 260 0Z M982 10Q930 10 888 -10.5Q846 -31 821.5 -65.5Q797 -100 797 -143Q797 -213 854 -254.5Q911 -296 1003 -310L1140 -330L1140 -351Q1140 -393 1119 -416Q1098 -439 1047 -439Q1011 -439 988.5 -424.5Q966 -410 955 -386Q947 -373 932 -373L853 -373Q842 -373 836.5 -379Q831 -385 831 -395Q832 -411 844.5 -434Q857 -457 882.5 -478.5Q908 -500 949 -515Q990 -530 1048 -530Q1111 -530 1154.5 -514.5Q1198 -499 1224 -473Q1250 -447 1262 -412Q1274 -377 1274 -338L1274 -24Q1274 -14 1267 -7Q1260 0 1250 0L1169 0Q1158 0 1151.5 -7Q1145 -14 1145 -24L1145 -63Q1132 -45 1110 -28Q1088 -11 1056.5 -0.5Q1025 10 982 10ZM1016 -85Q1051 -85 1079.5 -100Q1108 -115 1124.5 -146.5Q1141 -178 1141 -226L1141 -247L1041 -231Q982 -222 953 -202Q924 -182 924 -153Q924 -131 937.5 -115.5Q951 -100 972 -92.5Q993 -85 1016 -85Z M1397 0Q1387 0 1380 -7Q1373 -14 1373 -24L1373 -686Q1373 -696 1380 -703Q1387 -710 1397 -710L1480 -710Q1490 -710 1497 -703Q1504 -696 1504 -686L1504 -24Q1504 -14 1497 -7Q1490 0 1480 0Z M1632 0Q1622 0 1615 -7Q1608 -14 1608 -24L1608 -686Q1608 -696 1615 -703Q1622 -710 1632 -710L1715 -710Q1725 -710 1732 -703Q1739 -696 1739 -686L1739 -24Q1739 -14 1732 -7Q1725 0 1715 0Z M2066 10Q1955 10 1890 -54Q1825 -118 1820 -236Q1819 -246 1819 -261.5Q1819 -277 1820 -286Q1824 -362 1854.5 -416.5Q1885 -471 1939 -500.5Q1993 -530 2066 -530Q2147 -530 2202 -496.5Q2257 -463 2285 -403.5Q2313 -344 2313 -266L2313 -245Q2313 -235 2306 -228Q2299 -221 2288 -221L1956 -221Q1956 -221 1956 -218Q1956 -215 1956 -213Q1957 -178 1969.5 -148.5Q1982 -119 2006.5 -101Q2031 -83 2065 -83Q2093 -83 2112 -91.5Q2131 -100 2143 -111Q2155 -122 2160 -129Q2169 -141 2174.5 -143.5Q2180 -146 2191 -146L2277 -146Q2287 -146 2293.5 -140Q2300 -134 2299 -125Q2298 -109 2282.5 -86Q2267 -63 2238 -41Q2209 -19 2165.5 -4.5Q2122 10 2066 10ZM1956 -305L2177 -305L2177 -308Q2177 -347 2164 -376.5Q2151 -406 2126 -422.5Q2101 -439 2066 -439Q2031 -439 2006 -422.5Q1981 -406 1968.5 -376.5Q1956 -347 1956 -308Z M2622 0Q2562 0 2521 -21Q2480 -42 2460 -83.5Q2440 -125 2440 -187L2440 -414L2361 -414Q2351 -414 2344 -421Q2337 -428 2337 -439L2337 -496Q2337 -506 2344 -513Q2351 -520 2361 -520L2440 -520L2440 -686Q2440 -696 2446.5 -703Q2453 -710 2464 -710L2545 -710Q2555 -710 2562 -703Q2569 -696 2569 -686L2569 -520L2694 -520Q2704 -520 2711 -513Q2718 -506 2718 -496L2718 -439Q2718 -428 2711 -421Q2704 -414 2694 -414L2569 -414L2569 -197Q2569 -156 2583.5 -133Q2598 -110 2634 -110L2703 -110Q2713 -110 2720 -103Q2727 -96 2727 -86L2727 -24Q2727 -14 2720 -7Q2713 0 2703 0Z"

function Mark() {
  return (
    <>
      <rect className="logo-ink logo-stem" x="18.75" y="11" width="14" height="74" rx="3" />
      <path className="logo-ink" d={THREE} />
      <circle className="logo-node" cx="54.75" cy="62.5" r="6.5" />
    </>
  )
}

/**
 * The lockup, with the mark as the "B3": the mark stands at Rubik's cap
 * height (its 74-unit B3 on 700 font units, on the baseline), and "Wallet"
 * follows it at the space Rubik sets between a 3 and a W (52 units), so
 * it reads "B3Wallet". It is the design system's option W6 (2026-10-12),
 * redrawn with the official mark; the design system has no such file yet.
 */
export function Lockup() {
  return (
    <svg className="lockup" viewBox="0 -710 3336.823 720" width="83" height="18" aria-hidden="true">
      <g transform="translate(-177.365 -804.054) scale(9.4595)">
        <Mark />
      </g>
      <path className="logo-ink" transform="translate(567.378 0)" d={WALLET} />
    </svg>
  )
}

/** The mark alone, still, as `b3wallet-mark-*.svg`. */
export function B3Mark({ size = 16 }: { size?: number }) {
  return (
    <svg className="b3-mark" viewBox="0 0 96 96" width={size} height={size} aria-hidden="true">
      <Mark />
    </svg>
  )
}

/**
 * The animated mark the design system's `Loader` draws in B3Wallet: the node
 * breathes while the wallet works, and with `waiting` the stem blinks while
 * it waits for you. The motion is in app.css, since the CSP allows no style
 * inside the page.
 */
export function Loader({ waiting = false, size = 20 }: { waiting?: boolean; size?: number }) {
  return (
    <svg
      className={waiting ? "loader waiting" : "loader"}
      viewBox="0 0 96 96"
      width={size}
      height={size}
      aria-hidden="true"
    >
      <Mark />
    </svg>
  )
}
