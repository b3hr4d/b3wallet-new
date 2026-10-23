"""Computes the test vectors of docs/recovery.md section 8 again, from that
section's text alone, with nothing but Python's standard library, and checks
them against the digests it lists. A client in another language that matches
these computes what the recovery keys sign.

Run it with: python3 -I tools/recovery-tool/vectors.py
"""

import hashlib
import sys

SEED_KEYS = {
    1: "8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c",
    2: "8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394",
    3: "ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1",
    21: "d54207da194977dcf46adbfec2bc2e75b52d5a8a42184fedfdc00024f0e3e8da",
    22: "511c34a1a2cb521df16bb246b8de8e7997ce235c7e76b22a3d7503a24819dd8a",
    23: "31debe55d37c722768b137131caa6087080b2e0b60b94bd785d14575cfa498bc",
}
EXPECTED = {
    "policy": "f386a4d210393f76bdf24ea9f4303327f6896ffaf3bd1477b4b0902cf62d1fe1",
    "install": "3b7cc80f258157d2c400aa54f3715a326f4c9d2f5ee61344e0213e29f3c3250c",
    "release": "a1243672325ea5f24965f7d9301d48c5cc3ef084613d2a4e0362126aea40b584",
    "policy change": "905ed5a04f6fc07c386f7c422e6d7ca7ffa73d0c28e27bad6208880dd6799627",
    "replacement": "631db29dc67bd3a3c4259f56b6f538c25a5919346bee4fd21accb4437a8aaa96",
}

HOUR = 3600 * 10**9
DAY = 24 * HOUR
T0 = 1000 * DAY
CHUNK = 1 << 20


def sha256(data):
    return hashlib.sha256(data).digest()


def u8(value):
    return value.to_bytes(1, "big")


def u32(value):
    return value.to_bytes(4, "big")


def u64(value):
    return value.to_bytes(8, "big")


def blob(data):
    return u32(len(data)) + data


def items(values, encode):
    return u32(len(values)) + b"".join(encode(value) for value in values)


def domain(name):
    return name.encode("ascii") + b"\x00"


def key(seed):
    return bytes.fromhex(SEED_KEYS[seed])


def policy_body(policy):
    return (
        items(policy["keys"], lambda k: blob(k[0]) + blob(k[1].encode("utf-8")))
        + u8(policy["normal_threshold"])
        + u8(policy["sovereign_threshold"])
        + u64(policy["normal_delay"])
        + u64(policy["sovereign_delay"])
        + items(policy["signers"], blob)
        + u8(policy["release_threshold"])
    )


def policy_hash(policy):
    return sha256(domain("b3wallet-controller/policy") + policy_body(policy))


def operation_digest(op):
    out = domain("b3wallet-controller/operation") + u8(1)
    out += blob(op["controller"]) + blob(op["wallet"])
    out += u64(op["policy_version"]) + u64(op["nonce"]) + u64(op["expires_at"])
    kind, body = op["kind"]
    if kind == "install":
        out += u8(1)
        if body["mode"] == "reinstall":
            out += u8(2)
        else:
            out += u8(1) + u8(1 if body["skip_pre_upgrade"] else 0)
        wasm = body["wasm"]
        chunks = [wasm[i : i + CHUNK] for i in range(0, len(wasm), CHUNK)]
        out += blob(sha256(wasm))
        out += items([sha256(chunk) for chunk in chunks], blob)
        out += blob(sha256(body["arg"]))
        release = body["release"]
        if release is None:
            out += u8(0)
        else:
            out += u8(1)
            out += blob(release["version"].encode("utf-8"))
            out += u64(len(wasm))
            out += items(release["tested_from"], blob)
            out += blob(release["notes_url"].encode("utf-8"))
            out += u64(release["published_at"])
    elif kind == "set_policy":
        out += u8(2) + policy_body(body)
    elif kind == "replace":
        out += u8(3) + blob(body["controller"]) + blob(body["module_hash"])
        out += blob(body["policy_hash"])
    return sha256(out)


def main():
    policy = {
        "keys": [(key(seed), f"key {seed}") for seed in (1, 2, 3)],
        "normal_threshold": 1,
        "sovereign_threshold": 2,
        "normal_delay": 48 * HOUR,
        "sovereign_delay": 7 * DAY,
        "signers": [key(seed) for seed in (21, 22, 23)],
        "release_threshold": 2,
    }
    install_body = {
        "mode": "upgrade",
        "skip_pre_upgrade": False,
        "wasm": bytes(i % 256 for i in range(CHUNK + 10)),
        "arg": b"DIDL\x00\x00",
        "release": None,
    }
    base = {
        "controller": bytes.fromhex("00000000000000010101"),
        "wallet": bytes.fromhex("00000000000000020101"),
        "policy_version": 1,
        "nonce": 0,
        "expires_at": T0 + 10 * DAY,
    }
    install = dict(base, kind=("install", install_body))
    release = dict(
        base,
        kind=(
            "install",
            dict(
                install_body,
                release={
                    "version": "1.2.0",
                    "tested_from": [bytes([5]) * 32],
                    "notes_url": "https://example.org/1.2.0",
                    "published_at": T0,
                },
            ),
        ),
    )
    slower = dict(policy, sovereign_delay=14 * DAY)
    set_policy = dict(base, nonce=1, kind=("set_policy", slower))
    replace = dict(
        base,
        nonce=2,
        kind=(
            "replace",
            {
                "controller": bytes.fromhex("00000000000000030101"),
                "module_hash": bytes([1]) * 32,
                "policy_hash": policy_hash(policy),
            },
        ),
    )
    computed = {
        "policy": policy_hash(policy).hex(),
        "install": operation_digest(install).hex(),
        "release": operation_digest(release).hex(),
        "policy change": operation_digest(set_policy).hex(),
        "replacement": operation_digest(replace).hex(),
    }
    failed = False
    for name, digest in computed.items():
        ok = digest == EXPECTED[name]
        failed |= not ok
        print(f"{'ok  ' if ok else 'FAIL'} {name}: {digest}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
