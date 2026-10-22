import assert from "node:assert/strict"
import { createHash } from "node:crypto"
import { chmod, mkdtemp, readFile, stat, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { Ed25519KeyIdentity } from "@icp-sdk/core/identity"
import { readConfig } from "../src/config.ts"
import { joinCode } from "../src/join-code.ts"
import { loadOrCreateKey, pemOf, seedOf } from "../src/key.ts"

const seed = new Uint8Array(32).map((_, index) => index + 1)

test("a key file is the seed in PKCS #8 PEM, and reads back", () => {
  const pem = pemOf(seed)
  assert.match(
    pem,
    /^-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEI[A-Za-z0-9+/=]+\n-----END PRIVATE KEY-----\n$/,
  )
  assert.deepEqual(seedOf(pem), seed)
  assert.deepEqual(seedOf(pem.replace(/\n/g, "\r\n")), seed)
  for (const bad of [
    "",
    "nonsense",
    pem.replace("PRIVATE KEY", "PUBLIC KEY"),
    pemOf(seed).replace("MC4C", "MC4D"),
  ]) {
    assert.throws(() => seedOf(bad), /not an Ed25519 private key/)
  }
})

test("the first run makes a key only its owner can read, and later runs use it", async () => {
  const dir = await mkdtemp(join(tmpdir(), "b3wallet-mcp-"))
  const path = join(dir, "nested", "agent.pem")
  const first = await loadOrCreateKey(path)
  assert.equal(first.created, true)
  assert.equal((await stat(path)).mode & 0o777, 0o600)
  assert.equal((await stat(join(dir, "nested"))).mode & 0o777, 0o700)
  const again = await loadOrCreateKey(path)
  assert.equal(again.created, false)
  assert.equal(again.identity.getPrincipal().toText(), first.identity.getPrincipal().toText())
  const pem = await readFile(path, "utf8")
  assert.equal(
    Ed25519KeyIdentity.generate(seedOf(pem)).getPrincipal().toText(),
    first.identity.getPrincipal().toText(),
  )

  await chmod(path, 0o644)
  await assert.rejects(loadOrCreateKey(path), /others can read or write the key file/)
  await chmod(path, 0o600)
  await writeFile(path, "not a key")
  await assert.rejects(loadOrCreateKey(path), /not an Ed25519 private key/)
})

test("the configuration needs a wallet id, and names its pages", () => {
  assert.throws(() => readConfig({}), /set B3WALLET_ID/)
  assert.throws(() => readConfig({ B3WALLET_ID: "wallet" }), /not a canister id/)
  assert.throws(
    () => readConfig({ B3WALLET_ID: "aaaaa-aa", B3WALLET_NETWORK: "mainnet" }),
    /"ic" or "local"/,
  )
  const ic = readConfig({ B3WALLET_ID: " uxrrr-q7777-77774-qaaaq-cai ", B3WALLET_KEY: "/k.pem" })
  assert.deepEqual(ic, {
    wallet: "uxrrr-q7777-77774-qaaaq-cai",
    network: "ic",
    pages: "https://uxrrr-q7777-77774-qaaaq-cai.icp0.io",
    keyPath: "/k.pem",
  })
  const local = readConfig({
    B3WALLET_ID: "uxrrr-q7777-77774-qaaaq-cai",
    B3WALLET_NETWORK: "local",
  })
  assert.equal(local.pages, "http://uxrrr-q7777-77774-qaaaq-cai.localhost:8000")
  assert.match(local.keyPath, /\.config\/b3wallet-mcp\/agent\.pem$/)
})

test("a join code is the one the Approvals page shows", () => {
  const device = "gxhcg-wz6ja-zbwh7-s6ha4-nnlhc-ya5hh-2dv23-dwksw-ujozj-ymkcv-tae"
  const hash = createHash("sha256").update(device).digest()
  const digits = String(hash.readUInt32BE(0) % 100_000_000).padStart(8, "0")
  assert.equal(joinCode(device), `${digits.slice(0, 4)} ${digits.slice(4)}`)
})
