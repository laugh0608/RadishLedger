"""现有移动模拟器的本项目 App 控制；不创建设备，不处理秘密参数。"""
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import time

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
