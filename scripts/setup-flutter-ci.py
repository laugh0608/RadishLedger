#!/usr/bin/env python3
"""在 GitHub 临时目录准备与本地相同的 Flutter；不更新机器全局配置。"""
import os
from pathlib import Path
import subprocess

REVISION = "559ffa3f75e7402d65a8def9c28389a9b2e6fe42"


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("此入口只初始化 GitHub CI 临时 SDK；本地使用已安装的 Flutter")
    sdk = Path(os.environ["RUNNER_TEMP"]) / "radishledger-flutter"
    subprocess.run(["git", "clone", "--depth", "1", "--branch", "3.44.0",
                    "https://github.com/flutter/flutter.git", str(sdk)], check=True)
    actual = subprocess.check_output(["git", "-C", str(sdk), "rev-parse", "HEAD"], text=True).strip()
    if actual != REVISION:
        raise SystemExit("Flutter tag 与已审阅 commit 不一致")
    with Path(os.environ["GITHUB_PATH"]).open("a", encoding="utf-8") as stream:
        stream.write(str(sdk / "bin") + "\n")
    subprocess.run([str(sdk / "bin/flutter"), "--version"], check=True)


if __name__ == "__main__":
    main()
