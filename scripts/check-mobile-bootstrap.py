#!/usr/bin/env python3
"""复用现有模拟器，经私有 FIFO 验证 M0-C；秘密仅保留在控制器内存和管道。
先安装 bootstrap_main.dart --dart-define=M0C_DRIVER=true 的 debug 包。
不修改锁屏设置；不把核心 local-wrap 切换实验当成实际系统认证通过。
"""
import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import secrets
import shlex
import struct
import subprocess
import time
import uuid
from mobile_probe_device import Device

ROOT = Path(__file__).resolve().parent.parent


class BootstrapDevice(Device):
    def __init__(self, platform, device):
        super().__init__(platform, device)
        self.prefix = ("no_backup/m0-c/" if platform == "android"
                       else str(self.container / "Library/Application Support/m0-c") + "/")

    def launch_ready(self):
        self.stop()
        self.remove_result("control-ready.json")
        self.launch()
        return self.wait_json("control-ready.json", seconds=40)

    def pipe(self, name, data=None, seconds=35):
        path = self.prefix + name
        if self.platform == "android":
            base = [self.adb, "-s", self.device]
            base += ["shell", "-T"] if data is not None else ["exec-out"]
            base += ["run-as", self.package, "timeout", str(seconds)]
            command = (base + ["dd", "bs=1", "count=" + str(len(data)), "of=" + path, "status=none"]
                       if data is not None else base + ["cat", path])
            result = subprocess.run(command, input=data, capture_output=True, timeout=seconds + 5)
            if result.returncode != 0 or (data is not None and result.stdout):
                raise RuntimeError("Android 实验 FIFO 操作失败；不输出秘密通道内容")
            return result.stdout
        deadline = time.monotonic() + seconds
        flags = (os.O_WRONLY if data is not None else os.O_RDONLY) | os.O_NONBLOCK
        while True:
            try:
                descriptor = os.open(path, flags)
                break
            except OSError as error:
                if error.errno != errno.ENXIO or time.monotonic() >= deadline:
                    raise RuntimeError("iOS 实验 FIFO 无法打开") from None
                time.sleep(0.02)
        output = bytearray()
        offset = 0
        expected = None
        try:
            while time.monotonic() < deadline:
                try:
                    if data is not None:
                        offset += os.write(descriptor, data[offset:])
                        if offset == len(data):
                            return b""
                    else:
                        chunk = os.read(descriptor, 8192)
                        if chunk:
                            output.extend(chunk)
                            if len(output) >= 8:
                                meta_len, secret_len = struct.unpack(">II", output[:8])
                                if meta_len > 16384 or secret_len > 128:
                                    raise RuntimeError("秘密通道输出超限")
                                expected = 8 + meta_len + secret_len
                            if expected is not None and len(output) >= expected:
                                return output
                except BlockingIOError:
                    pass
                time.sleep(0.01)
            raise RuntimeError("实验 FIFO 超时")
        finally:
            os.close(descriptor)

    def send(self, bridge, run, op="core", secret=b"", **fields):
        metadata = json.dumps({"bridge": bridge, "run": run, "op": op, **fields}).encode()
        if len(metadata) > 4096 or len(secret) > 1104:
            raise RuntimeError("实验输入超限")
        frame = bytearray(struct.pack(">II", len(metadata), len(secret)))
        frame.extend(metadata)
        frame.extend(secret)
        try:
            self.pipe("control-in", frame)
        finally:
            frame[:] = b"\0" * len(frame)

    def call(self, bridge, run, op="core", secret=b"", **fields):
        self.send(bridge, run, op, secret, **fields)
        raw = bytearray(self.pipe("control-out"))
        try:
            if len(raw) < 8:
                raise RuntimeError("实验输出头不完整")
            length, secret_len = struct.unpack(">II", raw[:8])
            if length > 16384 or secret_len > 128 or len(raw) != 8 + length + secret_len:
                raise RuntimeError("实验输出长度不符")
            result = json.loads(raw[8:8 + length])
            secret = bytearray(raw[8 + length:])
            return result, secret
        finally:
            raw[:] = b"\0" * len(raw)

    def fingerprints(self, run):
        prefix = self.prefix + "m0c-" + run + "/"
        if self.platform == "ios":
            return {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                    for p in sorted(Path(prefix).iterdir()) if p.is_file()}
        result = self.shell("run-as", self.package, "sh", "-c",
                            shlex.quote("cd " + shlex.quote(prefix) + " && sha256sum *"))
        return {line.split()[1]: line.split()[0] for line in result.stdout.splitlines()}

    def file_sizes(self, run):
        prefix = self.prefix + "m0c-" + run + "/"
        if self.platform == "ios":
            return {p.name: p.stat().st_size for p in Path(prefix).iterdir() if p.is_file()}
        result = self.shell("run-as", self.package, "sh", "-c",
                            shlex.quote("cd " + shlex.quote(prefix) + " && stat -c '%n %s' *"))
        return {line.split()[0]: int(line.split()[1]) for line in result.stdout.splitlines()}


def success(response):
    result, secret = response
    if result.get("ok") is not True:
        secret[:] = b"\0" * len(secret)
        raise RuntimeError("实验调用失败：" + str(result.get("error", {}).get("code", "invalid_response")))
    return result, secret


def same_facts(before, after):
    for field in ["vault", "device", "audit", "contentDigest", "fixtureComplete", "ready"]:
        if before[field] != after[field]:
            raise RuntimeError("恢复前后事实不一致：" + field)


def nonempty_facts(report):
    """A11 人工预期；不能把旧 A01 或只有余额的报告当作本轮通过。"""
    audit = report["audit"]
    snapshot = audit["snapshot"]
    expected = (8, 8, 7, 3, 4, 1, 2, "18000", ["10000", "8000"])
    actual = (audit["sequence"], audit["revisions"], len(audit["operations"]),
              len(snapshot["activeEventIds"]), len(snapshot["selectedRevisionIds"]),
              len(snapshot["deletedEventIds"]), len(snapshot["unresolvedCandidateRevisionIds"]),
              snapshot["netAssetsMinor"], sorted(snapshot["balancesMinor"].values()))
    if actual != expected or report["ready"] is not True or report["fixtureComplete"] is not True:
        raise RuntimeError("A11 非空候选 / 删除 / outbox 与独立预期不符")


def file_fault_cases(device, bridge, report, save, cleanup):
    """设备上的真实文件发布路径；每次故障后重启进程，秘密不写报告。"""
    stages = ["shortWrite", "writeZero", "writeError", "fileSync", "publish", "directorySync"]

    def invoke(run, kind, secret=b"", **fields):
        return device.call(bridge, run, secret=secret, action={"kind": kind, **fields})

    def materials():
        run = uuid.uuid4().hex
        report["ownedRuns"].append(run)
        save()
        _, recovery = success(invoke(run, "newRecovery"))
        password = bytearray(secrets.token_hex(32).encode())
        return run, password, recovery, password + recovery

    def check_result(response, stage):
        result, secret = response
        if stage == "shortWrite":
            return success(response)
        secret[:] = b"\0" * len(secret)
        expected = "bootstrap_publish_uncertain" if stage == "directorySync" else "bootstrap_io"
        if result.get("ok") is not False or result.get("error", {}).get("code") != expected:
            raise RuntimeError("文件故障没有在预期边界拒绝：" + stage)
        return result, secret

    for file in ["bootstrap", "ready"]:
        for stage in stages:
            run, password, recovery, combined = materials()
            try:
                if file == "ready":
                    success(invoke(run, "create", combined))
                before_files = device.fingerprints(run) if file == "ready" else None
                action = {"kind": "create" if file == "bootstrap" else "confirm"}
                result, _ = check_result(device.call(bridge, run, secret=combined if file == "bootstrap" else recovery,
                    action=action, fileFault={"file": file, "stage": stage}), stage)
                pid = result["measurement"]["pid"]
                if before_files is not None and stage not in ["shortWrite", "directorySync"]:
                    assert before_files == device.fingerprints(run)
                device.launch_ready()
                incomplete = file == "bootstrap" and stage not in ["shortWrite", "directorySync"]
                if incomplete:
                    original = device.fingerprints(run)
                    assert "bootstrap.json" not in original and "ledger.db" not in original
                    rejected, _ = invoke(run, "resume", recovery, unlock="recovery")
                    assert rejected["ok"] is False and rejected["measurement"]["pid"] != pid
                    assert original == device.fingerprints(run)
                    duplicate, _ = invoke(run, "create", combined)
                    assert duplicate["error"]["code"] == "already_exists"
                    assert original == device.fingerprints(run)
                    actual = {"resumeRejected": True, "filesPreserved": True,
                              "pid": rejected["measurement"]["pid"]}
                else:
                    if file == "bootstrap" and stage == "directorySync":
                        assert "ledger.db" not in device.fingerprints(run)
                        success(invoke(run, "resume", recovery, unlock="recovery"))
                    if file == "ready":
                        checked, _ = success(invoke(run, "inspect", recovery, unlock="recovery"))
                        assert checked["report"]["ready"] == (stage in ["shortWrite", "directorySync"])
                    success(invoke(run, "confirm", recovery))
                    checked, _ = success(invoke(run, "seed", recovery, unlock="recovery"))
                    actual = checked["report"]
                    nonempty_facts(actual)
                    assert checked["measurement"]["pid"] != pid
                report["fileCases"].append({"bridge": bridge, "file": file, "stage": stage,
                    "beforePid": pid, "actual": actual, "result": "passed"})
                cleanup(bridge, run, incomplete=incomplete)
            finally:
                for material in [password, recovery, combined]: material[:] = b"\0" * len(material)
            print(f"{device.platform} {bridge} {file}/{stage}: file boundary passed", flush=True)

    run, password, recovery, combined = materials()
    local = bytearray()
    try:
        success(invoke(run, "create", combined))
        success(invoke(run, "confirm", recovery))
        seeded, _ = success(invoke(run, "seed", recovery, unlock="recovery"))
        baseline = seeded["report"]
        nonempty_facts(baseline)
        for file in ["localProtection", "activeProtection"]:
            for stage in stages:
                candidate, local = success(invoke(run, "prepareProtection", recovery, unlock="recovery"))
                generation = candidate["report"]["candidateProtection"]
                # 为活动引用覆盖建立完整旧代次；此前独立材料始终保留。
                success(invoke(run, "commitProtection", local, generation=generation))
                replacement, replacement_key = success(invoke(run, "prepareProtection", recovery, unlock="recovery"))
                target = replacement["report"]["candidateProtection"]
                written_key = bytearray()
                try:
                    before_files = device.fingerprints(run)
                    action = ({"kind": "prepareProtection", "unlock": "recovery"} if file == "localProtection"
                              else {"kind": "commitProtection", "generation": target})
                    response, written_key = check_result(device.call(bridge, run,
                        secret=recovery if file == "localProtection" else replacement_key,
                        action=action, fileFault={"file": file, "stage": stage}), stage)
                    pid = response["measurement"]["pid"]
                    if stage not in ["shortWrite", "directorySync"]:
                        assert before_files == device.fingerprints(run)
                    device.launch_ready()
                    opened, _ = success(invoke(run, "inspect", recovery, unlock="recovery"))
                    same_facts(baseline, opened["report"])
                    assert opened["measurement"]["pid"] != pid
                    expected_active = target if file == "activeProtection" and stage in ["shortWrite", "directorySync"] else generation
                    assert opened["report"]["activeProtection"] == expected_active
                    if file == "localProtection" and stage == "shortWrite":
                        success(invoke(run, "commitProtection", written_key, generation=response["report"]["candidateProtection"]))
                        verified, _ = success(invoke(run, "inspect", written_key, unlock="system"))
                        same_facts(baseline, verified["report"])
                    success(invoke(run, "commitProtection", replacement_key, generation=target))
                    device.launch_ready()
                    reopened, _ = success(invoke(run, "inspect", replacement_key, unlock="system"))
                    same_facts(baseline, reopened["report"])
                    assert reopened["measurement"]["pid"] != opened["measurement"]["pid"]
                    report["fileCases"].append({"bridge": bridge, "file": file, "stage": stage,
                        "beforePid": pid, "independentPid": opened["measurement"]["pid"],
                        "actual": reopened["report"], "result": "passed", "scope": "core local-wrap; no system authentication claim"})
                    save()
                finally:
                    for material in [local, replacement_key, written_key]: material[:] = b"\0" * len(material)
                print(f"{device.platform} {bridge} {file}/{stage}: nonempty recovery passed", flush=True)
        cleanup(bridge, run)
    finally:
        for material in [password, recovery, combined, local]: material[:] = b"\0" * len(material)


def wal_case(device, bridge, report, save, cleanup):
    run = uuid.uuid4().hex
    report["ownedRuns"].append(run)
    save()
    def invoke(kind, secret=b"", **fields):
        return device.call(bridge, run, secret=secret, action={"kind": kind, **fields})
    _, recovery = success(invoke("newRecovery"))
    _, wrong_recovery = success(invoke("newRecovery"))
    password = bytearray(secrets.token_hex(32).encode())
    combined = password + recovery
    try:
        created, _ = success(invoke("create", combined))
        success(invoke("confirm", recovery))
        device.send(bridge, run, secret=recovery, action={"kind": "seed", "unlock": "recovery"}, pauseAt="fixtureSaved")
        marker = device.wait_json(f"m0c-{run}/paused.json", seconds=35)
        assert marker["point"] == "fixtureSaved"
        sizes = device.file_sizes(run)
        assert sizes["ledger.db-wal"] > 32
        device.kill(marker["pid"])
        device.launch_ready()
        files = device.fingerprints(run)
        for unlock, material in [("password", b"incorrect synthetic input"), ("recovery", wrong_recovery)]:
            rejected, _ = invoke("inspect", material, unlock=unlock)
            assert rejected["ok"] is False and rejected["error"]["code"] == "authentication_failed"
            assert device.fingerprints(run) == files
        recovered, _ = success(invoke("inspect", recovery, unlock="recovery"))
        baseline = recovered["report"]
        nonempty_facts(baseline)
        for field in ["vault", "device"]: assert baseline[field] == created["report"][field]
        assert recovered["measurement"]["pid"] != marker["pid"]
        opened, _ = success(invoke("inspect", password, unlock="password"))
        same_facts(baseline, opened["report"])
        device.launch_ready()
        replayed, _ = success(invoke("replay", recovery, unlock="recovery"))
        same_facts(baseline, replayed["report"])
        assert replayed["measurement"]["pid"] != opened["measurement"]["pid"]
        report["coreCases"].append({"bridge": bridge, "scenario": "fixtureSaved", "signal": "SIGKILL",
            "killedPid": marker["pid"], "walBytes": sizes["ledger.db-wal"], "actual": baseline,
            "replayPid": replayed["measurement"]["pid"], "originalIntentReplay": True,
            "wrongPasswordAndRecoveryPreservedFiles": True})
        cleanup(bridge, run)
        print(f"{device.platform} {bridge}: A11 WAL kill / independent unlock / replay passed", flush=True)
    finally:
        for material in [password, recovery, combined, wrong_recovery]: material[:] = b"\0" * len(material)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=["android", "ios"])
    parser.add_argument("device")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "local-evidence/m0-c")
    args = parser.parse_args()
    evidence = args.output_dir.resolve() / (args.platform + "-bootstrap-report.json")
    if not evidence.parent.is_relative_to(ROOT / "local-evidence/m0-c"):
        raise RuntimeError("报告必须保留在项目 M0-C 忽略证据目录")
    if evidence.exists():
        previous = json.loads(evidence.read_text())
        if previous.get("ownedRuns"):
            raise RuntimeError("前次报告仍登记未清理对象；先核对精确身份并保留清理记录，不能覆盖")
    device = BootstrapDevice(args.platform, args.device)
    ready = device.launch_ready()
    report = {"schema": 2, "platform": args.platform, "device": args.device, "fixture": "A11",
              "mode": "debug Flutter / release Rust", "capabilities": ready["capabilities"],
              "coreCases": [], "fileCases": [], "platformCases": [], "measurements": [], "ownedRuns": [],
              "secretTransport": "private FIFO; controller memory only", "cleaned": False}
    evidence.parent.mkdir(parents=True, exist_ok=True)
    def save():
        evidence.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    def invoke(bridge, run, kind, secret=b"", **fields):
        return device.call(bridge, run, secret=secret, action={"kind": kind, **fields})
    def cleanup(bridge, run, incomplete=False):
        # 首次 bootstrap 发布前失败，没有执行过任何系统项操作；Rust 仍校验 owner。
        if incomplete:
            success(device.call(bridge, run, action={"kind": "cleanup"}))
        else:
            success(device.call(bridge, run, op="cleanup"))
        report["ownedRuns"].remove(run)
        save()
    try:
        for bridge in ["thin", "frb"]:
            run = uuid.uuid4().hex
            report["ownedRuns"].append(run)
            save()
            _, recovery = success(invoke(bridge, run, "newRecovery"))
            password = bytearray(secrets.token_hex(32).encode())
            combined = password + recovery
            local = bytearray()
            try:
                created, _ = success(invoke(bridge, run, "create", combined))
                report["measurements"].append({"bridge": bridge, "action": "create", **created["measurement"]})
                success(invoke(bridge, run, "confirm", recovery))
                seeded, _ = success(invoke(bridge, run, "seed", recovery, unlock="recovery"))
                baseline = seeded["report"]
                nonempty_facts(baseline)
                before_files = device.fingerprints(run)
                wrong, _ = invoke(bridge, run, "inspect", b"incorrect synthetic input", unlock="password")
                assert wrong["ok"] is False and wrong["error"]["code"] == "authentication_failed"
                assert before_files == device.fingerprints(run)
                killed = seeded["measurement"]["pid"]
                device.kill(killed)
                device.launch_ready()
                for unlock, material in [("password", password), ("recovery", recovery)]:
                    reopened, _ = success(invoke(bridge, run, "inspect", material, unlock=unlock))
                    same_facts(baseline, reopened["report"])
                    assert reopened["measurement"]["pid"] != killed
                    report["measurements"].append({"bridge": bridge, "action": unlock, **reopened["measurement"]})
                candidate, local = success(invoke(bridge, run, "prepareProtection", recovery, unlock="recovery"))
                generation = candidate["report"]["candidateProtection"]
                missing, _ = device.call(bridge, run, op="readProtection", generation=generation)
                assert missing["ok"] is False and missing["error"]["code"] == "item_missing"
                if ready["capabilities"]["supported"] is False:
                    rejected, _ = device.call(bridge, run, op="createProtection", generation=generation, secret=local)
                    assert rejected["ok"] is False and rejected["error"]["code"] == "unsupported"
                    report["platformCases"].append({"bridge": bridge, "creation": "unsupported",
                        "missingItem": missing["error"], "systemUnlockAndInvalidation": "not-executed",
                        "reason": "existing device authentication prerequisites unavailable; settings preserved"})
                else:
                    created_system, _ = device.call(bridge, run, op="createProtection", generation=generation, secret=local)
                    if created_system["ok"] is not True:
                        report["platformCases"].append({"bridge": bridge, "creation": created_system["error"],
                            "systemUnlockAndInvalidation": "not-executed", "missingItem": missing["error"]})
                    else:
                        _, retrieved = success(device.call(bridge, run, op="readProtection", generation=generation))
                        try:
                            success(invoke(bridge, run, "commitProtection", retrieved, generation=generation))
                        finally:
                            retrieved[:] = b"\0" * len(retrieved)
                        device.launch_ready()
                        system_open, _ = success(device.call(bridge, run, op="unlockSystem"))
                        same_facts(baseline, system_open["report"])
                        success(device.call(bridge, run, op="deleteProtection", generation=generation))
                        unavailable, _ = device.call(bridge, run, op="unlockSystem")
                        assert unavailable["ok"] is False and unavailable["error"]["code"] == "item_missing"
                        independent, _ = success(invoke(bridge, run, "inspect", recovery, unlock="recovery"))
                        same_facts(baseline, independent["report"])
                        success(device.call(bridge, run, op="establish", secret=recovery, unlock="recovery"))
                        device.launch_ready()
                        rebuilt, _ = success(device.call(bridge, run, op="unlockSystem"))
                        same_facts(baseline, rebuilt["report"])
                        report["platformCases"].append({"bridge": bridge, "creation": created_system["report"],
                            "restartUnlock": True, "exactItemDeletion": True, "independentRecovery": True,
                            "rebuiltRestartUnlock": True, "scope": "simulator API only; no device authentication strength claim"})
                recovered, _ = success(invoke(bridge, run, "inspect", recovery, unlock="recovery"))
                same_facts(baseline, recovered["report"])
                report["coreCases"].append({"bridge": bridge, "scenario": "normal-restart-and-wrong-password",
                    "killedPid": killed, "result": recovered["report"], "wrongMaterialPreservedFiles": True})
                cleanup(bridge, run)
            finally:
                for material in [password, recovery, combined, local]: material[:] = b"\0" * len(material)
            print(f"{args.platform} {bridge}: restart / independent unlock / wrong material passed", flush=True)
            for point in ["envelopesSaved", "databaseStarted", "databaseReady", "readyPublished", "protectionPrepared", "protectionPublished"]:
                run = uuid.uuid4().hex
                report["ownedRuns"].append(run)
                save()
                _, recovery = success(invoke(bridge, run, "newRecovery"))
                password = bytearray(secrets.token_hex(32).encode())
                combined = password + recovery
                local = bytearray()
                try:
                    if point in ["envelopesSaved", "databaseStarted", "databaseReady"]:
                        action, material = {"kind": "create"}, combined
                    else:
                        success(invoke(bridge, run, "create", combined))
                        if point == "readyPublished":
                            action, material = {"kind": "confirm"}, recovery
                        else:
                            success(invoke(bridge, run, "confirm", recovery))
                            before, _ = success(invoke(bridge, run, "seed", recovery, unlock="recovery"))
                            baseline = before["report"]
                            if point == "protectionPrepared":
                                action, material = {"kind": "prepareProtection", "unlock": "recovery"}, recovery
                            else:
                                candidate, local = success(invoke(bridge, run, "prepareProtection", recovery, unlock="recovery"))
                                action = {"kind": "commitProtection", "generation": candidate["report"]["candidateProtection"]}
                                material = local
                    device.send(bridge, run, action=action, secret=material, pauseAt=point)
                    marker = device.wait_json(f"m0c-{run}/paused.json", seconds=35)
                    assert marker["point"] == point
                    device.kill(marker["pid"])
                    device.launch_ready()
                    if point == "databaseStarted":
                        before_files = device.fingerprints(run)
                        failed, _ = invoke(bridge, run, "resume", recovery, unlock="recovery")
                        assert failed["ok"] is False and failed["error"]["code"] == "missing_database"
                        assert before_files == device.fingerprints(run)
                        actual = {"explicitFailure": failed["error"], "filesPreserved": True}
                    else:
                        resumed, _ = success(invoke(bridge, run, "resume", recovery, unlock="recovery"))
                        assert resumed["measurement"]["pid"] != marker["pid"]
                        success(invoke(bridge, run, "confirm", recovery))
                        after, _ = success(invoke(bridge, run, "seed", recovery, unlock="recovery"))
                        actual = after["report"]
                        if point in ["protectionPrepared", "protectionPublished"]:
                            same_facts(baseline, actual)
                        if point == "protectionPrepared":
                            assert actual["activeProtection"] is None
                        if point == "protectionPublished":
                            local_open, _ = success(invoke(bridge, run, "inspect", local, unlock="system"))
                            same_facts(baseline, local_open["report"])
                    report["coreCases"].append({"bridge": bridge, "scenario": point, "signal": "SIGKILL",
                        "killedPid": marker["pid"], "actual": actual,
                        "scope": "Rust bootstrap/local-wrap persistence; no platform authentication claim"})
                    cleanup(bridge, run)
                    print(f"{args.platform} {bridge} {point}: interruption boundary passed", flush=True)
                finally:
                    for material in [password, recovery, combined, local]: material[:] = b"\0" * len(material)
            wal_case(device, bridge, report, save, cleanup)
            file_fault_cases(device, bridge, report, save, cleanup)
        assert len(report["coreCases"]) == 16 and len(report["fileCases"]) == 48
        report["finalCapabilities"] = success(device.call("thin", uuid.uuid4().hex, op="capabilities"))[0]["report"]
        assert report["finalCapabilities"] == report["capabilities"]
        success(device.call("thin", uuid.uuid4().hex, op="stop"))
        report["cleaned"] = not report["ownedRuns"]
        save()
    finally:
        save()
        device.stop()


if __name__ == "__main__":
    main()
