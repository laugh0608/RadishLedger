#!/usr/bin/env python3
"""仅配合 bootstrap UI 集成测试，把明确模拟器内的本项目 App 退后台再返回。"""
import argparse
import json
import os
import re
import signal
import time

from mobile_probe_device import Device, run

IDENTITY = re.compile(r"[0-9a-f]{32}")


def request(value, session, bridge, selection):
    if (not isinstance(value, dict)
            or set(value) != {"session", "run", "bridge", "pid"}
            or value.get("session") != session or value.get("bridge") != bridge
            or not isinstance(value.get("run"), str) or not IDENTITY.fullmatch(value["run"])
            or type(value.get("pid")) is not int or value["pid"] <= 1
            or selection != {"run": value["run"], "bridge": bridge}):
        raise ValueError("本批后台请求或 selection 不符")
    return value


def read_project(device, name):
    if device.platform == "ios":
        path = device.container / "Library/Application Support/m0-c" / name
        if not path.exists():
            return None
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 2048:
            raise ValueError("本批观测文件不符合范围")
        return path.read_text()
    result = device.shell("run-as", device.package, "cat", "no_backup/m0-c/" + name, check=False)
    if result.returncode:
        if "No such file" in result.stderr or "No such file" in result.stdout:
            return None
        raise ValueError("无法读取本批 App 标记")
    if len(result.stdout.encode()) > 2048:
        raise ValueError("本批观测文件超过上限")
    return result.stdout


def foreground(device):
    if device.platform == "ios":
        run(["xcrun", "simctl", "launch", device.device, device.package])
    else:
        result = device.shell("am", "start", "-W", "-n", device.package + "/.MainActivity")
        if "Error:" in result.stdout:
            raise ValueError("App 返回前台失败")


def background_once(device, value):
    owner = read_project(device, "m0c-" + value["run"] + "/owner")
    if owner != "RadishLedger M0-C synthetic vault v1\n" + value["run"] + "\n":
        raise ValueError("本批账库 owner 不符")
    if device.platform == "ios":
        command = run(["ps", "-p", str(value["pid"]), "-o", "command="]).stdout.strip()
        if command != device.executable and not command.startswith(device.executable + " "):
            raise ValueError("本批 PID 不属于项目 App")
    elif str(value["pid"]) not in device.shell("pidof", device.package).stdout.split():
        raise ValueError("本批 PID 不属于项目 App")
    try:
        if device.platform == "ios":
            # 打开设置仅用来切换前台 App；不修改设置、不锁屏、不结束原进程。
            run(["xcrun", "simctl", "launch", device.device, "com.apple.Preferences"])
        else:
            device.shell("input", "keyevent", "KEYCODE_HOME")
        time.sleep(2)
    finally:
        foreground(device)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=["android", "ios"], required=True)
    parser.add_argument("--device", required=True)
    parser.add_argument("--session", required=True)
    args = parser.parse_args()
    if not IDENTITY.fullmatch(args.session):
        parser.error("必须提供明确的 32 位批次身份")
    # 宿主 driver 意外退出即停止；SIGTERM 仍执行本次前台恢复，不遗留监控服务。
    parent = os.getppid()
    def interrupted(*_):
        raise InterruptedError()
    signal.signal(signal.SIGTERM, interrupted)
    try:
        device = Device(args.platform, args.device)
        deadline = time.monotonic() + 600
        for bridge in ["thin", "frb"]:
            while time.monotonic() < deadline and os.getppid() == parent:
                raw = read_project(device, "ui-background-" + args.session + ".json")
                if raw is not None:
                    value = json.loads(raw)
                    if value.get("bridge") == bridge:
                        selection = json.loads(read_project(device, "selection.json") or "null")
                        background_once(device, request(value, args.session, bridge, selection))
                        break
                time.sleep(0.25)
            else:
                raise ValueError("等待本批 UI 请求超时或 driver 已退出")
        print("本批两个后台切换命令已执行；实际生命周期与 PID 由 App 独立断言。")
        return 0
    except Exception as error:
        # 原始命令输出 / 路径不进入诊断；真实失败类别保留，不制造默认通过。
        print("后台切换未完成：" + type(error).__name__)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
