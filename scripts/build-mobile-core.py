#!/usr/bin/env python3
"""为 Dart code asset 构建真实 Rust 目标；不安装工具或下载预编译业务库。"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent


def build(args):
    targets = {
        ("macos", "arm64", None): "aarch64-apple-darwin",
        ("linux", "x64", None): "x86_64-unknown-linux-gnu",
        ("android", "arm64", None): "aarch64-linux-android",
        ("android", "x64", None): "x86_64-linux-android",
        ("ios", "arm64", "iphoneos"): "aarch64-apple-ios",
        ("ios", "arm64", "iphonesimulator"): "aarch64-apple-ios-sim",
    }
    try:
        target = targets[(args.os, args.arch, args.ios_sdk)]
    except KeyError:
        raise SystemExit("目标没有接入验证入口；不能回退宿主库")
    output = Path(args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ, RUSTUP_TOOLCHAIN="1.96.0", CARGO_BUILD_JOBS="4")
    target_key = target.replace("-", "_")
    if args.os == "android":
        if not args.c_compiler or not args.minimum_version:
            raise SystemExit("Android 编译器与 API level 必须明确")
        binary_dir = Path(args.c_compiler).resolve().parent
        compiler = binary_dir / f"{target}{args.minimum_version}-clang"
        if not all(path.is_file() for path in [compiler, binary_dir / "llvm-ar", binary_dir / "llvm-ranlib"]):
            raise SystemExit("目标 NDK clang / ar / ranlib 不可用；不使用宿主工具代替")
        environment[f"CC_{target_key}"] = str(compiler)
        environment[f"AR_{target_key}"] = str(binary_dir / "llvm-ar")
        environment[f"RANLIB_{target_key}"] = str(binary_dir / "llvm-ranlib")
        environment[f"CARGO_TARGET_{target_key.upper()}_LINKER"] = str(compiler)
        environment["ANDROID_NDK_HOME"] = str(binary_dir.parent.parent.parent)
    if args.os == "ios":
        if not args.minimum_version:
            raise SystemExit("iOS deployment target 必须明确")
        environment["IPHONEOS_DEPLOYMENT_TARGET"] = args.minimum_version
        environment["SDKROOT"] = subprocess.check_output(
            ["xcrun", "--sdk", args.ios_sdk, "--show-sdk-path"], text=True
        ).strip()
    command = ["cargo", "build", "--locked", "--release", "--target", target,
               "-p", "radishledger-mobile"]
    if args.frb:
        command.extend(["--features", "frb"])
    subprocess.run(command, cwd=ROOT, env=environment, check=True)
    extension = "so" if args.os in {"android", "linux"} else "dylib"
    source = ROOT / "target" / target / "release" / f"libradishledger_mobile.{extension}"
    if not source.is_file():
        raise SystemExit("Rust 未生成所需库；不接受空资产")
    shutil.copyfile(source, output)
    if args.os == "ios":
        subprocess.run(["install_name_tool", "-id", "@rpath/libradishledger_mobile.dylib", str(output)], check=True)
    print(f"Rust asset: {target}; {source.stat().st_size} bytes")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--os", required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--ios-sdk")
    parser.add_argument("--minimum-version")
    parser.add_argument("--c-compiler")
    parser.add_argument("--output", required=True)
    parser.add_argument("--frb", action="store_true")
    build(parser.parse_args())


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        sys.exit(error.returncode)
