# 0009: uqr draws the receive page's QR codes

Status: proposed, 2026-10-22.

**Decision: the receive page draws each address's QR code with uqr
(`uqr`), pinned to the exact 0.1.3, and a test reads every QR code back with
an independent decoder before a release can build.**

A QR code is a second way to hand over an address, so it has to hold exactly
the address the page shows and copies: one that held another would send the
funds there. The encoder ships inside the signed release, like every other
dependency of the UI.

| Option | What it is | For | Against |
| --- | --- | --- | --- |
| **A. uqr 0.1.3** | unjs's QR encoder, MIT, a port of Project Nayuki's reference generator | No dependencies, no install scripts; a plain `encode(text)` that returns the modules, which the page draws as one SVG path; about 11 KB of `app.js` | A young version line (0.1.x) |
| B. qrcode-generator 2.0.4 | Kazuhiko Arase's encoder, MIT, in use since 2009 | Long record | Its own renderers and globals the page does not need, in a larger package |
| C. An encoder of our own | Reed-Solomon, masks and the layout, written here | No new dependency | About as much code as A, which we would have to get right ourselves |

How the page uses it, and what checks it:

- `ui/src/qr.ts` encodes the address text as given, at medium error
  correction with the four-module quiet zone, and turns the modules into a
  single SVG path. The page draws it dark on light in both themes, since
  scanners read that way round, with no style attribute and no new file.
- `ui/test/qr.test.ts` draws each QR code to pixels and reads it with jsQR
  (`jsqr` 1.4.0, Apache-2.0), a decoder that shares no code with uqr. It must
  read back exactly the address, for principals, ICRC-1 accounts with a
  subaccount and Bitcoin addresses, and the path must draw exactly the dark
  modules. jsQR is a dev dependency: it never ships.
- uqr was read before it was added: no network, DOM, `eval` or install
  script, and the installed files are the reviewed tarball's (same integrity
  hash in `ui/package-lock.json`).
