#!/usr/bin/env python3
"""在已启动、已安装 recovery_main 的现有模拟器上验证真实进程强杀恢复。
仅操作本项目 App 的合成 fixture；不创建设备，不处理密钥或清其他 App 数据。
"""
import argparse
import json
from pathlib import Path
import uuid
from mobile_probe_device import COMMAND, RESULT, Device

ROOT = Path(__file__).resolve().parent.parent

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=["android", "ios"])
    parser.add_argument("device")
    args = parser.parse_args()
    device = Device(args.platform, args.device)
    for name in [COMMAND, RESULT, RESULT + ".tmp"]:
        if device.read(name) is not None:
            raise RuntimeError("已有实验控制文件，需先确认归属；不覆盖：" + name)
    evidence = ROOT / "local-evidence/m0-b" / (args.platform + "-recovery-report.json")
    report = {"schema": 1, "platform": args.platform, "device": args.device,
              "mode": "debug Flutter / release Rust", "cases": []}
    evidence.parent.mkdir(parents=True, exist_ok=True)
    owned = False
    try:
        for bridge in ["thin", "frb"]:
            for point in ["beforeOutbox", "beforeCommit", "afterCommit"]:
                identity = uuid.uuid4().hex
                device.remove_result()
                device.write_command({"bridge": bridge, "action": {
                    "kind": "recoveryStart", "run": identity, "point": point}})
                owned = True
                device.launch()
                marker = device.wait_json(f"radishledger-recovery-{identity}/ready.json")
                assert marker["run"] == identity and marker["point"] == point
                device.kill(marker["pid"])
                device.write_command({"bridge": bridge, "action": {
                    "kind": "recoveryInspect", "run": identity, "point": point}})
                device.launch()
                result = device.wait_json(RESULT)
                if result.get("ok") is not True:
                    raise RuntimeError(json.dumps(result))
                actual = result["report"]
                assert actual["pid"] != marker["pid"] and actual["pid"] == result["runnerPid"]
                assert actual["before"]["sequence"] == (3 if point == "afterCommit" else 2)
                assert actual["after"]["sequence"] == 3
                assert actual["duplicateVerified"] and actual["changedIntentRejected"]
                report["cases"].append({"bridge": bridge, "point": point, "signal": "SIGKILL",
                                        "killedPid": marker["pid"], "result": actual})
                evidence.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
                device.remove_result()
                device.write_command({"bridge": bridge, "action": {
                    "kind": "recoveryCleanup", "run": identity}})
                device.launch()
                cleanup = device.wait_json(RESULT)
                assert cleanup.get("ok") is True and cleanup["report"]["cleaned"] is True
                print(f"{args.platform} {bridge} {point}: kill / recovery / replay / cleanup passed", flush=True)
        device.remove_result()
        device.remove_result(COMMAND)
        report["cleaned"] = True
        evidence.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    finally:
        if owned:
            device.stop()
        # 失败时保留本次合成文件供核对，不凭猜测清理未完成事实。


if __name__ == "__main__":
    main()
