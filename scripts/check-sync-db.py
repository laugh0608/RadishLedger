#!/usr/bin/env python3
"""显式运行 M0-D 的真实 PostgreSQL 实验；不属于普通只读仓库检查。"""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import signal
import subprocess
import sys
import time
import uuid
from datetime import datetime, timezone

from sync_db_probe import (
    ROOT, SCOPE, D, E, P, Q, SEED, Z, Database, check, expected, operation,
    sql_text, submit,
)


B0_HEADS = {"X": ("x0", False), "Y": ("y0", False)}
P_HEADS = {"X": ("x1", False), "Y": ("y1", False)}


def page(applied=1, upper=2, budget=4096, pause="", restore="R1"):
    return (f"SELECT read_page({SCOPE},{sql_text(restore)},{applied},{upper},"
            f"{budget},{sql_text(pause)});")


def freeze(identity="freeze-1", restore="R1"):
    return f"SELECT freeze_domain({SCOPE},{sql_text(restore)},{sql_text(identity)});"


def unfreeze(identity="freeze-1", restore="R1"):
    return f"SELECT unfreeze_domain({SCOPE},{sql_text(restore)},{sql_text(identity)});"


def verify(db: Database):
    def atomic():
        before = db.state()
        gate, writer = db.gate(), db.connect()
        ticket = writer.send(submit(P, "head"))
        db.blocked(writer, gate)
        check(db.state(), before)
        db.release(gate)
        check(json.loads(writer.finish(ticket)), expected(2, "x1", "y1"))
        db.assert_business(2, P_HEADS, ["op-seed", "op-p"])
        check(db.state()["batches"][1]["body"], P)
    db.case("S01-atomic-visible", atomic)

    def cas(read_only):
        op = copy.deepcopy(P)
        if read_only:
            op["pre"]["Z"] = "z0"
        else:
            check(db.observer.value(submit(E)), expected(2, "xe"))
        before = db.state()
        check(db.observer.value(submit(op)), {"status": "preconditionRejected"})
        after = db.state()
        check({k: v for k, v in after.items() if k != "receipts"},
              {k: v for k, v in before.items() if k != "receipts"})
        check(len(after["receipts"]), len(before["receipts"]) + 1)
        check(db.observer.value(submit(op)), {"status": "preconditionRejected"})
        check(db.state(), after)
        altered = copy.deepcopy(op)
        altered["nonce"] = "changed"
        db.rejected(submit(altered), "identity_reuse")
        check(db.state(), after)
    for read_only in [False, True]:
        db.case(f"S02-CAS-read-only-{read_only}", lambda ro=read_only: cas(ro))

    def structural():
        before = db.state()
        for variant in ["duplicate", "missing", "contradiction"]:
            op = copy.deepcopy(P)
            if variant == "duplicate":
                op["writes"].append(op["writes"][0])
            elif variant == "missing":
                del op["pre"]["X"]
            else:
                op["writes"][0]["parent"] = "wrong"
            db.rejected(submit(op), "duplicate" if variant == "duplicate" else "structure")
            check(db.state(), before)
    db.case("S02-structure", structural)

    def replay(policy, reason):
        check(db.observer.value(submit(P)), expected(2, "x1", "y1"))
        check(db.observer.value(submit(Q)), expected(3, "x2"))
        db.observer.run(policy)
        before = db.state()
        fresh = db.connect()
        check(fresh.value(submit(P)), expected(2, "x1", "y1"))
        db.rejected(submit(Z), reason)
        check(db.state(), before)
    for name, policy, reason in [
        ("freeze", freeze(), "frozen"),
        ("quota", "UPDATE domains SET business_limit=0;", "business_limit"),
        ("epoch", "UPDATE domains SET epoch=2;", "epoch"),
    ]:
        db.case("S03-replay-" + name, lambda p=policy, r=reason: replay(p, r))

    def identity():
        db.observer.run(submit(P))
        before = db.state()
        for variant in ["nonce", "payload", "parent", "members"]:
            op = copy.deepcopy(P)
            if variant == "nonce":
                op["nonce"] = "changed"
            elif variant == "payload":
                op["writes"][0]["payload"] = "changed"
            elif variant == "parent":
                op["pre"]["X"] = op["writes"][0]["parent"] = "x1"
            else:
                op["writes"].pop()
            db.rejected(submit(op), "identity_reuse")
            check(db.state(), before)
        reused = operation("op-reused", {"X": "x1"}, [("X", "x1", False)])
        db.rejected(submit(reused), "revision_reuse")
        check(db.state(), before)
    db.case("S04-identity-binding", identity)

    def concurrent(changed):
        a, b = db.connect(), db.connect()
        check(a.value("BEGIN; " + submit(P)), expected(2, "x1", "y1"))
        other = copy.deepcopy(P)
        if changed:
            other["nonce"] = "different"
        ticket = b.send(submit(other))
        db.blocked(b, a)
        db.assert_business(1, B0_HEADS, ["op-seed"])
        a.run("COMMIT;")
        if changed:
            try:
                b.finish(ticket)
            except RuntimeError as error:
                if "ERROR:  identity_reuse" not in str(error):
                    raise
            else:
                raise AssertionError("concurrent altered request accepted")
        else:
            check(json.loads(b.finish(ticket)), expected(2, "x1", "y1"))
        db.assert_business(2, P_HEADS, ["op-seed", "op-p"])
        check(len(db.state()["receipts"]), 2)
    for changed in [False, True]:
        db.case(f"S05-concurrent-altered-{changed}", lambda c=changed: concurrent(c))

    def uncertain():
        writer = db.connect()
        writer.run("BEGIN; " + submit(P))
        check(len(db.state()["receipts"]), 1)
        # 未读取 COMMIT 的输出；另一连接看到回执后终止原 backend，重连原内容。
        writer.send("COMMIT;")
        deadline = time.monotonic() + 5
        while len(db.state()["receipts"]) != 2:
            if time.monotonic() > deadline:
                raise TimeoutError("commit was not observed")
            time.sleep(0.02)
        db.kill_backend(writer)
        fresh = db.connect()
        check(fresh.value(submit(P)), expected(2, "x1", "y1"))
        db.assert_business(2, P_HEADS, ["op-seed", "op-p"])
    db.case("S06-database-commit-disconnect", uncertain)

    def ordering(commit):
        a, b = db.connect(), db.connect()
        a.run("BEGIN; " + submit(Z))
        ticket = b.send(submit(P))
        db.blocked(b, a)
        db.assert_business(1, B0_HEADS, ["op-seed"])
        a.run("COMMIT;" if commit else "ROLLBACK;")
        check(json.loads(b.finish(ticket)), expected(3 if commit else 2, "x1", "y1"))
        db.assert_business(3 if commit else 2,
                           {**P_HEADS, "Z": ("z0", False)} if commit else P_HEADS,
                           ["op-seed", "op-z", "op-p"] if commit else ["op-seed", "op-p"])
    for commit in [True, False]:
        db.case(f"S07-late-writer-commit-{commit}", lambda c=commit: ordering(c))

    def wrong_sequence():
        db.observer.run("CREATE TABLE sequence_control(position bigint PRIMARY KEY); "
                        "CREATE SEQUENCE wrong_counter START 2;")
        a, b = db.connect(), db.connect()
        check(a.run("BEGIN; INSERT INTO sequence_control VALUES(nextval('wrong_counter')) "
                    "RETURNING position;"), "2")
        check(b.run("INSERT INTO sequence_control VALUES(nextval('wrong_counter')) "
                    "RETURNING position;"), "3")
        check(db.observer.value("SELECT jsonb_agg(position) FROM sequence_control;"), [3])
        a.run("COMMIT;")
        check(db.observer.value("SELECT jsonb_agg(position ORDER BY position) FROM sequence_control;"), [2, 3])
        check(db.observer.value("SELECT count(*) FROM sequence_control WHERE position>3;"), 0)
        db.observer.run("DROP TABLE sequence_control; DROP SEQUENCE wrong_counter;")
    db.case("S07-negative-sequence-missed-position", wrong_sequence)

    def interrupted(stage):
        before = db.state()
        gate, writer = db.gate(), db.connect()
        ticket = writer.send(submit(P, stage))
        db.blocked(writer, gate)
        check(db.state(), before)
        db.kill_backend(writer)
        try:
            writer.finish(ticket)
        except RuntimeError as error:
            if "terminating connection due to administrator command" not in str(error):
                raise
        else:
            raise AssertionError("terminated backend unexpectedly finished")
        db.release(gate)
        check(db.state(), before)
        check(db.connect().value(submit(P)), expected(2, "x1", "y1"))
        db.assert_business(2, P_HEADS, ["op-seed", "op-p"])
    for stage in ["head", "batch", "receipt", "water"]:
        db.case("S08-terminate-after-" + stage, lambda s=stage: interrupted(s))

    def database_crash():
        db.observer.run(submit(Z))
        before = db.state()
        writer = db.connect()
        writer.run("BEGIN; " + submit(P))
        check(db.state(), before)
        command(["docker", "kill", "--signal=KILL", db.container])
        db.close()
        command(["docker", "start", db.container])
        wait_ready(db.container)
        db.observer = db.connect()
        check(db.state(), before)
        check(db.observer.value(submit(P)), expected(3, "x1", "y1"))
        db.assert_business(3, {**P_HEADS, "Z": ("z0", False)}, ["op-seed", "op-z", "op-p"])
    db.case("S08-postmaster-kill-recovery", database_crash)

    def pages():
        db.observer.run(submit(P) + submit(Z))
        upper = db.observer.value(f"SELECT high_water FROM domains WHERE scope={SCOPE};")
        check(upper, 3)
        check(db.observer.value(page(1, upper)), {"position": 2, "body": P})
        db.observer.run(submit(Q))
        check(db.observer.value(page(2, upper)), {"position": 3, "body": Z})
        check(db.observer.value(page(3, upper)), {"done": True})
        check(db.observer.value(page(1, upper)), {"position": 2, "body": P})
        check(db.observer.value(page(3, 4)), {"position": 4, "body": Q})
        for a, u in [(4, 3), (1, 5), (-1, 3)]:
            db.rejected(page(a, u), "window")
        db.observer.run("DELETE FROM batches WHERE position=2;")
        db.rejected(page(1, 3), "gap")
    db.case("S09-fixed-window-history-gap", pages)

    def read_restore():
        db.observer.run(submit(P))
        gate, reader, restore = db.gate(), db.connect(), db.connect()
        read_ticket = reader.send(page(pause="page"))
        db.blocked(reader, gate)
        restore_ticket = restore.send("UPDATE domains SET restore='R2';")
        db.blocked(restore, reader)
        db.release(gate)
        check(json.loads(reader.finish(read_ticket)), {"position": 2, "body": P})
        restore.finish(restore_ticket)
        db.rejected(page(), "restore")
        db.rejected(submit(P), "restore")
        db.rejected(freeze(), "restore")
    db.case("S09-read-restore-lock-only", read_restore)

    def limits():
        db.observer.run(submit(P))
        size = db.observer.value("SELECT octet_length(body::text) FROM batches WHERE position=2;")
        before = db.state()
        db.rejected(page(budget=size - 1), "page_limit")
        for budget in [size, size + 1]:
            check(db.observer.value(page(budget=budget)), {"position": 2, "body": P})
        check(db.state(), before)
    db.case("S10-page-budget-boundary", limits)

    def admission_boundary(kind, count):
        op = operation("limit-op", {f"O{i}": None for i in range(count if kind != "bytes" else 1)},
                       [(f"O{i}", f"r{i}", False) for i in range(count if kind == "writes" else 1)])
        if kind == "bytes":
            op["padding"] = ""
            size = db.observer.value(f"SELECT octet_length({sql_text(json.dumps(op))}::jsonb::text);")
            op["padding"] = "a" * (count - size)
        before = db.state()
        maximum = {"writes": 4, "pre": 8, "bytes": 4096}[kind]
        if count > maximum:
            db.rejected(submit(op), "body_limit" if kind == "bytes" else "member_limit")
            check(db.state(), before)
        else:
            result = db.observer.value(submit(op))
            check(result, expected(2, *(f"r{i}" for i in range(count if kind == "writes" else 1))))
    for kind, maximum in [("writes", 4), ("pre", 8), ("bytes", 4096)]:
        for count in [maximum - 1, maximum, maximum + 1]:
            db.case(f"S10-experimental-{kind}-{count}", lambda k=kind, n=count: admission_boundary(k, n))

    def overflow():
        db.observer.run("UPDATE domains SET high_water=9223372036854775807, "
                        "business_limit=9223372036854775807;")
        before = db.state()
        db.rejected(submit(P), "overflow")
        check(db.state(), before)
    db.case("S10-counter-overflow", overflow)

    def deletion():
        check(db.observer.value(submit(D)), expected(2, "xd"))
        check(db.observer.value(submit(E)), {"status": "preconditionRejected"})
        create = operation("create-again", {"X": None}, [("X", "new", False)])
        check(db.observer.value(submit(create)), {"status": "preconditionRejected"})
        db.assert_business(2, {"X": ("xd", True), "Y": ("y0", False)}, ["op-seed", "op-delete"])
        restore = operation("restore-x", {"X": "xd"}, [("X", "xr", False)])
        check(db.observer.value(submit(restore)), expected(3, "xr"))
        check(next(v for v in db.state()["revisions"] if v["revision"] == "xd")["body"]["deleted"], True)
    db.case("S16-database-tombstone", deletion)

    def isolation():
        db.observer.run(submit(P))
        before = db.state()
        for original in ["instance-A", "account-A", "vault-A", "V1"]:
            scope = SCOPE.replace(original, original + "-other")
            db.observer.run(f"INSERT INTO domains(scope) VALUES({scope});")
            check(db.observer.value(submit(SEED, scope=scope)), expected(1, "x0", "y0"))
            check(db.observer.value(submit(P, scope=scope)), expected(2, "x1", "y1"))
            check(db.state(), before)
            check(len(db.state(scope)["receipts"]), 2)
    db.case("S17-storage-scope-only", isolation)

    def freeze_race(write_first):
        a, b = db.connect(), db.connect()
        a.run("BEGIN; " + (submit(P) if write_first else freeze()))
        ticket = b.send(freeze() if write_first else submit(P))
        db.blocked(b, a)
        a.run("COMMIT;")
        if write_first:
            check(b.finish(ticket), "2")
            db.assert_business(2, P_HEADS, ["op-seed", "op-p"])
        else:
            try:
                b.finish(ticket)
            except RuntimeError as error:
                if "ERROR:  frozen" not in str(error):
                    raise
            else:
                raise AssertionError("write passed freeze")
            db.assert_business(1, B0_HEADS, ["op-seed"])
        before = db.state()
        check(db.observer.run(freeze()), "2" if write_first else "1")
        check(db.observer.value(submit(P if write_first else SEED)),
              expected(2, "x1", "y1") if write_first else expected(1, "x0", "y0"))
        check(db.state(), before)
    for write_first in [True, False]:
        db.case(f"S21-freeze-write-first-{write_first}", lambda w=write_first: freeze_race(w))

    def cancellation():
        check(db.observer.run(freeze()), "1")
        check(db.observer.run(unfreeze()), "cancelled")
        check(db.observer.run(unfreeze()), "already_cancelled")
        check(db.observer.run(freeze("freeze-2")), "1")
        before = db.state()
        check(db.observer.run(unfreeze()), "already_cancelled")
        check(db.observer.run(freeze()), "1")
        check(db.state(), before)
        db.observer.run("UPDATE domains SET restore='R2';")
        before = db.state()
        db.rejected(unfreeze("freeze-2"), "restore")
        check(db.state(), before)
    db.case("S22-conditional-unfreeze-only", cancellation)

    def permission():
        control, writer = db.connect(), db.connect()
        control.run("BEGIN; UPDATE domains SET allowed=false;")
        ticket = writer.send(submit(P))
        db.blocked(writer, control)
        control.run("COMMIT;")
        try:
            writer.finish(ticket)
        except RuntimeError as error:
            if "ERROR:  access" not in str(error):
                raise
        else:
            raise AssertionError("queued write ignored revoked control flag")
        before = db.state()
        for sql in [submit(SEED), page(1, 1), freeze(), unfreeze()]:
            db.rejected(sql, "access")
        check(db.state(), before)
        db.assert_business(1, B0_HEADS, ["op-seed"])
    db.case("S24-queued-control-flag-only", permission)

    def receipt_budget():
        db.observer.run(submit(E) + "UPDATE domains SET receipt_limit=3;")
        check(db.observer.value(submit(P)), {"status": "preconditionRejected"})
        before = db.state()
        other = copy.deepcopy(P)
        other["id"] = "new-rejection"
        db.rejected(submit(other), "receipt_limit")
        db.rejected(freeze(), "receipt_limit")
        check(db.observer.value(submit(P)), {"status": "preconditionRejected"})
        check(db.state(), before)
    db.case("S24-receipt-budget", receipt_budget)


def command(args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True, timeout=30, **kwargs).stdout.strip()


def wait_ready(container):
    deadline = time.monotonic() + 30
    while True:
        ready = subprocess.run(["docker", "exec", container, "pg_isready", "-U", "postgres"],
                               capture_output=True, timeout=5)
        # entrypoint 的初始化临时 server 也会 ready；PID 1 必须已 exec postgres。
        pid1 = command(["docker", "exec", container, "cat", "/proc/1/comm"])
        if ready.returncode == 0 and pid1 == "postgres":
            return
        if time.monotonic() > deadline:
            raise TimeoutError("PostgreSQL startup deadline")
        time.sleep(0.1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", default="postgres:17.10-alpine", help="必须已缓存；运行时固定为本地 image ID，不 pull")
    args = parser.parse_args()
    session = uuid.uuid4().hex
    container = "radishledger-sync-db-" + session[:12]
    directory = ROOT / "local-evidence" / "m0-d" / session
    if not directory.resolve().is_relative_to(ROOT):
        raise ValueError("evidence path leaves repository")
    image = json.loads(command(["docker", "image", "inspect", args.image]))[0]
    directory.mkdir(parents=True, exist_ok=False)
    report = {"session": session, "container": container, "imageId": image["Id"],
              "imageDigests": image.get("RepoDigests", []), "architecture": image["Architecture"],
              "startedAt": datetime.now(timezone.utc).isoformat(), "cases": [], "barriers": [],
              "head": command(["git", "rev-parse", "HEAD"]),
              "sourceSha256": {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in [
                  "scripts/check-sync-db.py", "scripts/sync_db_probe.py", "scripts/sync-db-probe.sql"]},
              "scope": "synthetic-database-only", "passed": False, "cleaned": False}
    db = None
    created = False
    failure = None

    def interrupted(signum, _frame):
        raise RuntimeError(f"interrupted by signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    (directory / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print("Owned experiment: " + container + "; evidence: " + str(directory), flush=True)
    try:
        # create 超时也可能已生效；进入精确归属核对，不能遗漏不确定的创建结果。
        created = True
        command(["docker", "create", "--name", container, "--label", "radishledger.sync-probe=" + session,
                 "--pull=never", "--network=none", "--cpus=2", "--memory=512m", "--pids-limit=128",
                 "-e", "POSTGRES_HOST_AUTH_METHOD=trust", image["Id"],
                 "postgres", "-c", "listen_addresses=", "-c", "max_connections=24"])
        command(["docker", "start", container])
        wait_ready(container)
        db = Database(container, report)
        report["version"] = db.observer.run("SHOW server_version;")
        report["isolation"] = db.observer.run("SHOW transaction_isolation;")
        check(report["isolation"], "read committed")
        report["durability"] = db.observer.value(
            "SELECT jsonb_build_object('fsync',current_setting('fsync'),"
            "'synchronous_commit',current_setting('synchronous_commit'),"
            "'full_page_writes',current_setting('full_page_writes'));")
        check(report["durability"], {"fsync": "on", "synchronous_commit": "on", "full_page_writes": "on"})
        db.observer.run((ROOT / "scripts/sync-db-probe.sql").read_text())
        verify(db)
        report["passed"] = True
    except (Exception, KeyboardInterrupt) as error:
        failure = error
        report["error"] = str(error)
    finally:
        if db:
            try:
                db.close()
            except Exception as error:
                report["sessionCleanupError"] = str(error)
                failure = failure or error
        if created:
            try:
                info = json.loads(command(["docker", "inspect", container]))[0]
                check(info["Config"]["Labels"].get("radishledger.sync-probe"), session)
                check(info["Image"], image["Id"])
                report["containerId"] = info["Id"]
                report["volumes"] = [m["Name"] for m in info["Mounts"] if m["Type"] == "volume"]
                command(["docker", "rm", "-f", "-v", info["Id"]])
                remaining = command(["docker", "ps", "-aq", "--filter", "id=" + info["Id"]])
                check(remaining, "")
                volumes = command(["docker", "volume", "ls", "-q"]).splitlines()
                check(set(report["volumes"]) & set(volumes), set())
                report["cleaned"] = True
            except Exception as error:
                report["cleanupError"] = str(error)
                failure = failure or error
        report["passed"] = report["passed"] and failure is None and report["cleaned"]
        report["completedAt"] = datetime.now(timezone.utc).isoformat()
        (directory / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
        print("Evidence: " + str(directory / "report.json"), flush=True)
    if failure:
        print("FAILED: " + str(failure), file=sys.stderr)
        return 1
    print(f"Passed {len(report['cases'])} database cases; owned container and volumes removed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
