#!/usr/bin/env python3
"""只复核公开评审输入、字节布局与预检调度；不执行 KDF / AEAD 或生成归档。"""
from __future__ import annotations

import hashlib
import json
import runpy
import struct
import sys
from pathlib import Path


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def unique_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate material field")
        result[key] = value
    return result


def preflight(prefix: bytes, actual_length: int, *, expected_minor: int = 1) -> str:
    """独立解读已给定的 128 字节公开前缀，不接受文件 / 密文或执行解密。"""
    if len(prefix) != 128 or prefix[:8] != b"RLM0E001":
        return "invalid_archive"
    major, minor, header_size, suite, profile, flags = struct.unpack_from(
        ">HHIHHI", prefix, 8
    )
    if (major, minor) != (0, expected_minor) or suite != 1 or flags != 0:
        return "unsupported_archive"
    if header_size != 176:
        return "invalid_archive"
    if profile != 1 or struct.unpack_from(">IIII", prefix, 56) != (19, 65536, 3, 1):
        return "unsupported_kdf"
    length = struct.unpack_from(">Q", prefix, 120)[0]
    if not 153 <= length <= 16777368:
        return "archive_limit"
    if actual_length != 176 + length:
        return "invalid_archive"
    return "framing_only"


def check(material: dict) -> None:
    require(material["materialVersion"] == 2, "unsupported material version")
    require(material["profile"] == "m0-archive-container-review-1", "review profile drift")
    require(material["dataOrigin"] == "synthetic-public-review-inputs", "not public material")
    require(
        material["status"] == "approved_host_experiment_not_frozen",
        "review status drift",
    )
    proposal = material["proposal"]
    require(
        proposal == {
            "headerBytes": 176, "manifestBytes": 116, "plaintextOverheadBytes": 136,
            "tagBytes": 16, "fileOverheadBytes": 328, "maxPayloadBytes": 16777216,
            "maxFileBytes": 16777544, "maxRecords": 100000, "maxJsonDepth": 64,
            "maxStringBytes": 65536, "maxDecodedStringBytes": 16777216,
            "maxCollectionEntries": 1000000, "maxConcurrentKdf": 1,
        },
        "proposal limits drift",
    )
    base = material["baseInput"]
    require(base["passwordUtf8"] == "public-review-password", "public password input drift")
    require(bytes.fromhex(base["archiveKeyHex"]) == bytes(range(128, 160)), "public key input drift")
    require(base["kdf"] == {
        "algorithm": "argon2id", "version": 19, "memoryKiB": 65536,
        "iterations": 3, "lanes": 1, "outputBytes": 32,
    }, "KDF input drift")
    payload = base["payloadUtf8"].encode("utf-8")
    require(len(payload) == 518, "handwritten payload length drift")
    value = json.loads(payload, object_pairs_hook=unique_object)
    require(value["facts"]["profile"] == "m0-logical-snapshot-v1", "payload profile drift")
    require(value["facts"]["remoteKnowledge"] == "unknown", "remote scope drift")
    require(value["facts"]["imports"] is None, "unexpected source context")
    require(value["verification"]["netAssetsMinor"] == "0", "empty fixture summary drift")

    expected = material["framingExpected"]
    prefix = bytes.fromhex(expected["headerPrefixHex"])
    require(len(prefix) == 128, "header prefix length drift")
    require(prefix[24:40] == bytes.fromhex(base["archiveIdHex"]) == bytes(range(16)), "archive ID drift")
    require(prefix[40:56] == bytes.fromhex(base["saltHex"]) == bytes(range(16, 32)), "salt drift")
    require(prefix[72:96] == bytes.fromhex(base["wrapNonceHex"]) == bytes(range(64, 88)), "wrap nonce drift")
    require(prefix[96:120] == bytes.fromhex(base["contentNonceHex"]) == bytes(range(96, 120)), "content nonce drift")
    require(preflight(prefix, 846) == "framing_only", "base preflight material mismatch")
    require(expected["payloadBytes"] == 518, "payload byte count mismatch")
    require(expected["payloadSha256"] == hashlib.sha256(payload).hexdigest(), "payload SHA mismatch")

    manifest = bytes.fromhex(expected["manifestHex"])
    require(len(manifest) == 116 and manifest[:8] == b"RLM0MAN1", "manifest prefix mismatch")
    require(struct.unpack_from(">HHIQ", manifest, 8) == (1, 1, 1, 0), "manifest version / source mismatch")
    require(struct.unpack_from(">13I", manifest, 24) == (0,) * 13, "manifest counts mismatch")
    require(struct.unpack_from(">Q", manifest, 76)[0] == 518, "manifest payload length mismatch")
    require(manifest[84:116] == hashlib.sha256(payload).digest(), "manifest digest mismatch")
    plaintext = (116).to_bytes(4, "big") + manifest + (518).to_bytes(8, "big") + payload + b"RLM0END1"
    require(len(plaintext) == expected["plaintextBytes"] == 654, "plaintext size mismatch")
    require(hashlib.sha256(plaintext).hexdigest() == expected["plaintextSha256"], "plaintext SHA mismatch")
    require(expected["contentCipherBytes"] == 670 and expected["fileBytes"] == 846, "output size arithmetic mismatch")

    domains = material["domains"]
    require(domains == {
        "wrapKeyInfoAscii": "radishledger/archive/review1/password-wrap",
        "contentKeyInfoAscii": "radishledger/archive/review1/content-key",
        "wrapAadLabelAscii": "radishledger/archive/review1/wrap",
        "contentAadLabelAscii": "radishledger/archive/review1/content",
        "aadSeparatorHex": "00",
    }, "domain labels drift")
    require(
        bytes.fromhex(expected["wrapAadHex"]) == domains["wrapAadLabelAscii"].encode("ascii") + b"\0" + prefix,
        "wrap AAD mismatch",
    )
    crypto = material["cryptoExpected"]
    require(crypto["status"] == "independent_go_oracle", "unverified crypto status")
    require(set(crypto) == {
        "status", "argon2idOutputHex", "wrapKeyHex", "contentKeyHex",
        "wrappedArchiveKeyHex", "contentCiphertextHex", "archiveSha256",
        "archiveHex", "m0cPasswordKeyHex", "unicodeNfcArgon2Hex", "unicodeNfdArgon2Hex",
        "headerPrefixHex", "manifestHex", "plaintextSha256", "provenance",
    }, "crypto expected fields drift")
    public_archive = bytes.fromhex(crypto["archiveHex"])
    require(len(public_archive) == 846, "oracle archive length drift")
    require(public_archive[:128] == prefix, "oracle header mismatch")
    require(public_archive[128:176] == bytes.fromhex(crypto["wrappedArchiveKeyHex"]), "oracle key envelope mismatch")
    require(public_archive[176:] == bytes.fromhex(crypto["contentCiphertextHex"]), "oracle body mismatch")
    require(hashlib.sha256(public_archive).hexdigest() == crypto["archiveSha256"] == "59816737fb2759dde36bc75dc222124f74430a984b221225fdfa93dba75fc478", "independent archive digest drift")
    for key in ("argon2idOutputHex", "wrapKeyHex", "contentKeyHex", "m0cPasswordKeyHex", "unicodeNfcArgon2Hex", "unicodeNfdArgon2Hex"):
        require(len(bytes.fromhex(crypto[key])) == 32, "oracle key length drift")
    for key in ("headerPrefixHex", "manifestHex", "plaintextSha256"):
        require(crypto[key] == expected[key], "independent framing drift")
    provenance = crypto["provenance"]
    source = runpy.run_path(str(Path(__file__).with_name("archive-vector-oracle.py")))["SOURCE"]
    require(hashlib.sha256(source.encode()).hexdigest() == provenance["sourceSha256"] == "91cebb3caa80d3f2fa7dc6b06770fda034533418c268071f1cc335c54f9c0f5d", "oracle source drift")
    require(provenance["goVersion"] == "go version go1.26.3 darwin/arm64", "oracle toolchain provenance drift")
    require(provenance["modules"] == {"golang.org/x/crypto":"v0.50.0", "golang.org/x/sys":"v0.43.0"}, "oracle module versions drift")
    original = {k:v for k,v in crypto.items() if k != "provenance"}
    original_bytes = (json.dumps(original,sort_keys=True,indent=2)+"\n").encode()
    require(hashlib.sha256(original_bytes).hexdigest() == provenance["outputsSha256"] == "09ab0c5fc94b3b495a5f8c2a212c75de303a0190f025da23aaabfac3b05232ef", "independent oracle outputs drift")

    cases = material["cases"]
    require([c["id"] for c in cases] == [f"R{n:02}" for n in range(1, 25)], "review case IDs drift")
    require(all(c["execution"] == "mapped_to_host_tests" and c.get("hostTest") for c in cases), "host coverage mapping drift")
    mutations = {
        "R02": (8, ">H", 1, "major=1"),
        "R03": (12, ">I", 4294967295, "header_length=4294967295"),
        "R04": (16, ">H", 2, "suite_id=2"),
        "R05": (18, ">H", 2, "kdf_profile_id=2"),
        "R06": (60, ">I", 65535, "memory_kib=65535"),
        "R07": (60, ">I", 4294967295, "memory_kib=4294967295"),
        "R08": (68, ">I", 4, "lanes=4"),
        "R09": (20, ">I", 1, "flags=1"),
        "R10": (120, ">Q", 16777369, "content_cipher_length=16777369"),
    }
    checked = 0
    for case in cases:
        candidate = bytearray(prefix)
        actual = 846
        if case["id"] in mutations:
            offset, form, replacement, description = mutations[case["id"]]
            require(case["mutation"] == description, "mutation description drift")
            struct.pack_into(form, candidate, offset, replacement)
        elif case["id"] in ("R11", "R12"):
            delta = -1 if case["id"] == "R11" else 1
            require(case["mutation"] == f"actual_file_length=expected{delta:+}", "file length mutation drift")
            actual += delta
        else:
            continue
        require(case["stage"] == "preflight", "preflight stage drift")
        require(preflight(candidate, actual) == case["expected"], "preflight expectation mismatch")
        checked += 1
    require(checked == 11, "preflight case coverage drift")


def read_material(path: Path) -> dict:
    with path.open("rb") as source:
        raw = source.read(65537)
    require(len(raw) <= 65536, "review material too large")
    return json.loads(raw, object_pairs_hook=unique_object)


def check_restored(path: Path) -> None:
    material = path / "archive-restored-review-input.json"
    data = read_material(material)
    artifact = read_material(path / "archive-restored-review-vector.json")
    base, output, provenance = data["baseInput"], artifact["output"], artifact["provenance"]
    require(data["dataOrigin"] == "synthetic-public-restored-review-inputs", "restored origin drift")
    require(output["status"] == "independent_go_oracle", "restored vector provenance drift")
    payload = base["payloadUtf8"].encode("utf-8")
    archive = bytes.fromhex(output["archiveHex"])
    prefix = archive[:128]
    require(preflight(prefix, len(archive)) == "unsupported_archive", "legacy version drift")
    require(preflight(prefix, len(archive), expected_minor=2) == "framing_only", "restored header drift")
    require(len(archive) == len(payload) + 328, "restored archive size drift")
    for field, start, end in [("archiveIdHex", 24, 40), ("saltHex", 40, 56),
                              ("wrapNonceHex", 72, 96), ("contentNonceHex", 96, 120)]:
        require(prefix[start:end] == bytes.fromhex(base[field]), "restored entropy drift")
    manifest = bytes.fromhex(output["manifestHex"])
    expected = b"RLM0MAN1" + struct.pack(">HHIQ13IQ", 1, 2, 2, 1,
                                        1, 0, 2, 2, 0, 2, *([0] * 7), len(payload))
    require(manifest == expected + hashlib.sha256(payload).digest(), "restored manifest drift")
    body = struct.pack(">I", 116) + manifest + struct.pack(">Q", len(payload)) + payload + b"RLM0END1"
    require(hashlib.sha256(body).hexdigest() == output["plaintextSha256"], "restored plaintext drift")
    require(hashlib.sha256(archive).hexdigest() == output["archiveSha256"], "restored archive digest drift")
    require(hashlib.sha256(material.read_bytes()).hexdigest() == provenance["materialSha256"], "restored material drift")
    oracle = Path(__file__).with_name("archive-vector-oracle.py")
    sources = runpy.run_path(str(oracle))
    legacy = sources["SOURCE"].replace("func main() {", "func legacyMain() {", 1)
    source = legacy + "\0" + sources["RESTORED_SOURCE"]
    require(hashlib.sha256(source.encode()).hexdigest() == provenance["sourceSha256"], "restored oracle source drift")
    output_bytes = (json.dumps(output, sort_keys=True, indent=2) + "\n").encode()
    require(hashlib.sha256(output_bytes).hexdigest() == provenance["outputsSha256"], "restored oracle output drift")


def main() -> int:
    path = Path(__file__).resolve().parents[1] / "docs/validation"
    try:
        check(read_material(path / "archive-container-review-inputs.json"))
        check_restored(path)
    except (OSError, ValueError, KeyError, TypeError, struct.error):
        print("archive review: material validation failed", file=sys.stderr)
        return 1
    print("archive review: 0.1 / 0.2 framing, offsets, SHA, 11 preflight schedules and 24 case declarations checked")
    print("independent Go crypto outputs and source pinned; this check does not run KDF / AEAD / restore")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
