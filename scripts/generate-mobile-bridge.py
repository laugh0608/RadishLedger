#!/usr/bin/env python3
"""固定版本与项目内工具生成 FRB；缺少工具时停止，不自动全局安装。"""
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent


def main():
    tools = ROOT / "local-data/tools/bin"
    for executable, version in [("flutter_rust_bridge_codegen", "2.13.0"), ("cargo-expand", "1.0.118")]:
        binary = tools / executable
        if not binary.is_file():
            raise SystemExit(f"缺少项目内生成工具：{executable} {version}")
        actual = subprocess.check_output([str(binary), "--version"], text=True).strip()
        if version not in actual:
            raise SystemExit(f"生成工具版本不匹配：{executable}")
    environment = dict(os.environ, RUSTUP_TOOLCHAIN="1.96.0")
    environment["PATH"] = str(tools) + os.pathsep + environment.get("PATH", "")
    environment["CARGO_INSTALL_ROOT"] = str(ROOT / "local-data/tools")
    subprocess.run([
        str(tools / "flutter_rust_bridge_codegen"), "generate",
        "--rust-input", "crate::api", "--rust-root", "crates/ledger-mobile",
        "--rust-output", "crates/ledger-mobile/src/frb_generated.rs",
        "--dart-output", "apps/m0_probe/lib/src/frb_generated", "--dart-root", "apps/m0_probe",
        "--no-web", "--no-deps-check", "--no-auto-upgrade-dependency",
        "--no-add-mod-to-lib", "--stop-on-error",
    ], cwd=ROOT, env=environment, check=True)
    # 生成器的默认 rustfmt edition 与 workspace 2024 不同，统一到正式格式入口。
    subprocess.run(["cargo", "fmt", "--all"], cwd=ROOT, env=environment, check=True)


if __name__ == "__main__":
    main()
