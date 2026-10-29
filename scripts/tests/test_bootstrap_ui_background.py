"""后台请求归属与取消后的前台恢复；不访问设备。"""
import importlib.util
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("bootstrap_ui_background", ROOT / "scripts/observe-bootstrap-ui-background.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class BackgroundRequestTest(unittest.TestCase):
    def test_foreign_or_unbounded_request_rejected(self):
        value = {"session": "1" * 32, "run": "2" * 32, "bridge": "thin", "pid": 42}
        selected = {"run": value["run"], "bridge": "thin"}
        self.assertEqual(MODULE.request(value, value["session"], "thin", selected), value)
        for change in [{"session": "3" * 32}, {"run": "../other"}, {"pid": True}, {"pid": 0},
                       {"bridge": "frb"}, {"secret": "must-not-be-accepted"}]:
            with self.assertRaises(ValueError):
                MODULE.request({**value, **change}, value["session"], "thin", selected)
        with self.assertRaises(ValueError):
            MODULE.request(value, value["session"], "thin", {**selected, "cleanupCandidates": []})

    def test_interruption_returns_owned_app_to_foreground(self):
        class FakeDevice:
            platform = "android"
            package = "synthetic.package"
            calls = []
            def shell(self, *args):
                self.calls.append(args)
                class Result:
                    stdout = "42"
                return Result()
        device = FakeDevice()
        value = {"run": "2" * 32, "pid": 42}
        owner = "RadishLedger M0-C synthetic vault v1\n" + value["run"] + "\n"
        with patch.object(MODULE, "read_project", return_value=owner), patch.object(MODULE.time, "sleep", side_effect=InterruptedError):
            with self.assertRaises(InterruptedError):
                MODULE.background_once(device, value)
        self.assertEqual(device.calls[-1], ("am", "start", "-W", "-n", "synthetic.package/.MainActivity"))
        self.assertIn(("input", "keyevent", "KEYCODE_HOME"), device.calls)
