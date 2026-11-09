"""有界合成分片的离线重组；不启动 adb / 模拟器，不接触系统密钥。"""
import base64
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("bootstrap_evidence", ROOT / "scripts/collect-bootstrap-evidence.py")
COLLECTOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(COLLECTOR)
SESSION, REPORT = "1" * 32, "2" * 32


def document():
    return {"schema": 1, "scope": "synthetic-bootstrap-observations", "session": SESSION,
            "reportId": REPORT, "hardwareVerified": False, "acceptance": "not-assessed",
            "omittedObservations": 0, "observations": [], "syntheticPadding": "x" * 1600}


def frames(value=None, raw=None):
    payload = raw if raw is not None else json.dumps(value or document()).encode()
    encoded = base64.b64encode(payload).decode()
    chunks = [encoded[i:i + 700] for i in range(0, len(encoded), 700)]
    return [f"I/flutter (123): RLM0_BOOTSTRAP:{SESSION}:{REPORT}:{i}:{len(chunks)}:{chunk}"
            for i, chunk in enumerate(chunks)]


class BootstrapEvidenceTest(unittest.TestCase):
    def test_reordered_identical_retry_and_foreign_report(self):
        source = frames()
        text = "\n".join([*reversed(source), source[0], source[0].replace(REPORT, "3" * 32)])
        self.assertEqual(COLLECTOR.collect(text, SESSION, REPORT), document())

    def test_missing_truncated_conflicting_and_malformed_frames_reject(self):
        source = frames()
        for bad in [source[:-1], [source[0][:-1], *source[1:]],
                    [*source, source[0][:-1] + ("A" if source[0][-1] != "A" else "B")],
                    [*source, f"RLM0_BOOTSTRAP:{SESSION}:{REPORT}:999:999:AAAA"],
                    [*source, f"RLM0_BOOTSTRAP:{SESSION}:{REPORT}:broken"]]:
            with self.subTest(case=bad[-1][:90]):
                with self.assertRaises(ValueError):
                    COLLECTOR.collect("\n".join(bad), SESSION, REPORT)

    def test_payload_json_identity_and_evidence_boundaries_reject(self):
        for raw in [b'not json', b'{"schema":1,"schema":1}', b'\xff', b'{"value":NaN}', b'{"value":1e309}']:
            with self.assertRaises(ValueError):
                COLLECTOR.collect("\n".join(frames(raw=raw)), SESSION, REPORT)
        for key, value in [("schema", True), ("session", "3" * 32), ("reportId", "4" * 32),
                           ("hardwareVerified", True), ("acceptance", "passed"),
                           ("observations", [{}] * 65), ("omittedObservations", -1)]:
            report = document()
            report[key] = value
            with self.subTest(key=key):
                with self.assertRaises(ValueError):
                    COLLECTOR.collect("\n".join(frames(report)), SESSION, REPORT)

    def test_byte_and_identity_limits(self):
        with self.assertRaises(ValueError):
            COLLECTOR.collect("x" * (COLLECTOR.MAX_INPUT + 1), SESSION, REPORT)
        with self.assertRaises(ValueError):
            COLLECTOR.collect("\n".join(frames(raw=b"x" * (COLLECTOR.MAX_PAYLOAD + 1))), SESSION, REPORT)
        with self.assertRaises(ValueError):
            COLLECTOR.collect("\n".join(frames()), "../../foreign", REPORT)

    def test_exclusive_output_paths_and_failed_input_preserve_evidence(self):
        with tempfile.TemporaryDirectory(prefix="radishledger-evidence-test-") as temp:
            previous = COLLECTOR.ROOT
            self.addCleanup(setattr, COLLECTOR, "ROOT", previous)
            COLLECTOR.ROOT = Path(temp)
            root = Path(temp) / "local-evidence/m0-c"
            root.mkdir(parents=True)
            source, output = root / "filtered.log", root / "report.json"
            source.write_text("\n".join(frames()))
            self.assertEqual(COLLECTOR.collect_file(source, output, SESSION, REPORT), (0, 0))
            saved = output.read_bytes()
            with self.assertRaises(ValueError):
                COLLECTOR.collect_file(source, output, SESSION, REPORT)
            self.assertEqual(output.read_bytes(), saved)
            outside = Path(temp) / "outside.json"
            with self.assertRaises(ValueError):
                COLLECTOR.collect_file(source, outside, SESSION, REPORT)
            source.write_text("incomplete synthetic log")
            failed = root / "failed.json"
            with self.assertRaises(ValueError):
                COLLECTOR.collect_file(source, failed, SESSION, REPORT)
            self.assertFalse(failed.exists())
            source.write_bytes(b'\xff')
            with self.assertRaisesRegex(ValueError, "UTF-8"):
                COLLECTOR.collect_file(source, failed, SESSION, REPORT)
            with self.assertRaisesRegex(ValueError, "普通"):
                COLLECTOR.collect_file(root, failed, SESSION, REPORT)
            if hasattr(os, "mkfifo"):
                pipe = root / "not-a-log"
                os.mkfifo(pipe)
                with self.assertRaisesRegex(ValueError, "普通"):
                    COLLECTOR.collect_file(pipe, failed, SESSION, REPORT)


if __name__ == "__main__":
    unittest.main()
