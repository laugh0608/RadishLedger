#!/usr/bin/env python3
"""在已启动、已安装 recovery_main 的现有模拟器上验证真实进程强杀恢复。
仅操作本项目 App 的合成 fixture；不创建设备，不处理密钥或清其他 App 数据。
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
COMMAND = "m0-recovery-command.json"
RESULT = "m0-recovery-result.json"


def run(command, *, check=True, input=None):
    return subprocess.run(command, input=input, capture_output=True, text=True,
                          check=check, timeout=30)


class Device:
    def __init__(self, platform, device):
        self.platform, self.device = platform, device
        self.package = ("com.radishledger.radishledger_probe" if platform == "android"
                        else "com.radishledger.radishledgerProbe")
        if platform == "android":
            self.adb = str(Path(os.environ.get("ANDROID_HOME", str(Path.home() / "Library/Android/sdk"))) / "platform-tools/adb")
            if not device.startswith("emulator-"):
                raise RuntimeError("本入口只允许 Android 模拟器")
            assert self.shell("getprop", "ro.kernel.qemu").stdout.strip() == "1"
            self.prefix = "code_cache/"
        else:
            states = json.loads(run(["xcrun", "simctl", "list", "devices", "--json"]).stdout)
            matches = [d for ds in states["devices"].values() for d in ds if d["udid"] == device]
            if len(matches) != 1 or matches[0]["state"] != "Booted":
                raise RuntimeError("必须指定已启动的确切 iOS Simulator")
            data = run(["xcrun", "simctl", "get_app_container", device, self.package, "data"]).stdout.strip()
            self.container = Path(data).resolve()
            self.prefix = str(self.container / "tmp") + "/"
            self.executable = run(["xcrun", "simctl", "get_app_container", device, self.package, "app"]).stdout.strip() + "/Runner"

    def shell(self, *arguments, **kwargs):
        return run([self.adb, "-s", self.device, "shell", *arguments], **kwargs)

    def read(self, name):
        if self.platform == "ios":
            p = Path(self.prefix + name)
            return p.read_text() if p.exists() else None
        result = self.shell("run-as", self.package, "cat", self.prefix + name, check=False)
        if result.returncode != 0:
            if "No such file" in result.stderr or "No such file" in result.stdout:
                return None
            raise RuntimeError("无法读取本项目合成实验文件")
        return result.stdout

    def write_command(self, command):
        data = json.dumps(command)
        if self.platform == "ios":
            Path(self.prefix + COMMAND).write_text(data)
        else:
            self.shell("run-as", self.package, "sh", "-c",
                       shlex.quote("cat > " + self.prefix + COMMAND), input=data)

    def remove_result(self, name=RESULT):
        if self.platform == "ios":
            Path(self.prefix + name).unlink(missing_ok=True)
        else:
            self.shell("run-as", self.package, "rm", "-f", self.prefix + name)

    def stop(self):
        if self.platform == "ios":
            result = run(["xcrun", "simctl", "terminate", self.device, self.package], check=False)
            if result.returncode and "not running" not in result.stderr and "found nothing to terminate" not in result.stderr:
                raise RuntimeError(result.stderr)
        else:
            self.shell("am", "force-stop", self.package)

    def launch(self):
        self.stop()
        if self.platform == "ios":
            run(["xcrun", "simctl", "launch", self.device, self.package])
        else:
            result = self.shell("am", "start", "-W", "-n", self.package + "/.MainActivity")
            if "Error:" in result.stdout:
                raise RuntimeError(result.stdout)

    def kill(self, pid):
        if not isinstance(pid, int) or pid <= 1:
            raise RuntimeError("无效的目标 PID")
        if self.platform == "ios":
            command = run(["ps", "-p", str(pid), "-o", "command="]).stdout.strip()
            if command != self.executable and not command.startswith(self.executable + " "):
                raise RuntimeError("PID 不属于本项目 Simulator App")
            os.kill(pid, signal.SIGKILL)
        else:
            pids = self.shell("pidof", self.package).stdout.split()
            if str(pid) not in pids:
                raise RuntimeError("PID 不属于本项目 Android App")
            self.shell("run-as", self.package, "kill", "-9", str(pid))

    def wait_json(self, name, seconds=25):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            content = self.read(name)
            if content:
                try:
                    return json.loads(content)
                except json.JSONDecodeError:
                    pass  # marker 写入完成前允许下一次有界读取。
            time.sleep(0.1)
        raise RuntimeError("等待合成实验结果超时：" + name)


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
