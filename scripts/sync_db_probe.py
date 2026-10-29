"""隔离 PostgreSQL 实验的进程、连接与确定性锁屏障。仅使用合成输入。"""

from __future__ import annotations

import json
import queue
import subprocess
import threading
import time
import uuid
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SCOPE = "ARRAY['instance-A','account-A','vault-A','V1']"


def sql_text(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def operation(identity: str, pre: dict, changes: list[tuple]) -> dict:
    return {
        "id": identity, "epoch": 1, "nonce": "synthetic-" + identity,
        "pre": pre,
        "writes": [
            {"id": obj, "rev": revision, "parent": pre[obj],
             "deleted": deleted, "payload": "synthetic-" + revision}
            for obj, revision, deleted in changes
        ],
    }


SEED = operation("op-seed", {"X": None, "Y": None}, [("X", "x0", False), ("Y", "y0", False)])
P = operation("op-p", {"X": "x0", "Y": "y0"}, [("X", "x1", False), ("Y", "y1", False)])
Q = operation("op-q", {"X": "x1"}, [("X", "x2", False)])
Z = operation("op-z", {"Z": None}, [("Z", "z0", False)])
E = operation("op-edit", {"X": "x0"}, [("X", "xe", False)])
D = operation("op-delete", {"X": "x0"}, [("X", "xd", True)])


def submit(op: dict, pause: str = "", scope: str = SCOPE, restore: str = "R1") -> str:
    return (f"SELECT submit({scope}, {sql_text(restore)}, "
            f"{sql_text(json.dumps(op))}::jsonb, {sql_text(pause)});")


def expected(position: int, *revisions: str) -> dict:
    return {"status": "committed", "position": position, "revisions": list(revisions)}


def check(actual, wanted) -> None:
    if actual != wanted:
        raise AssertionError(f"actual={actual!r}; expected={wanted!r}")


class Session:
    def __init__(self, db: "Database"):
        self.db = db
        self.lines: queue.Queue = queue.Queue()
        self.process = subprocess.Popen(
            ["docker", "exec", "-i", db.container, "psql", "-X", "-qAt",
             "-v", "ON_ERROR_STOP=1", "-P", "pager=off", "-U", "postgres"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            text=True, bufsize=1,
        )
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        db.sessions.append(self)
        self.run("SET lock_timeout='10s'; SET statement_timeout='15s'; "
                 "SET idle_in_transaction_session_timeout='30s';")
        self.pid = int(self.run("SELECT pg_backend_pid();"))

    def _read(self):
        for line in self.process.stdout:
            self.lines.put(line.rstrip("\n"))
        self.lines.put(None)

    def send(self, sql: str) -> str:
        marker = "done_" + uuid.uuid4().hex
        self.process.stdin.write(sql + "\n\\echo " + marker + "\n")
        self.process.stdin.flush()
        return marker

    def finish(self, marker: str) -> str:
        result = []
        deadline = time.monotonic() + 18
        while time.monotonic() < deadline:
            line = self.lines.get(timeout=max(0.01, deadline - time.monotonic()))
            if line is None:
                raise RuntimeError("psql exited: " + "\n".join(result))
            if line == marker:
                return "\n".join(line for line in result if line)
            result.append(line)
        raise TimeoutError("psql response deadline")

    def run(self, sql: str) -> str:
        return self.finish(self.send(sql))

    def value(self, sql: str):
        return json.loads(self.run(sql))

    def close(self):
        if self.process.stdin and not self.process.stdin.closed:
            try:
                self.process.stdin.close()
            except BrokenPipeError:
                pass  # 强杀 / 已拒绝的连接已关闭；仍回收本机子进程。
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=2)
        self.reader.join(timeout=2)
        self.process.stdout.close()


class Database:
    def __init__(self, container: str, report: dict):
        self.container = container
        self.report = report
        self.sessions: list[Session] = []
        self.observer = self.connect()

    def connect(self) -> Session:
        return Session(self)

    def reset(self):
        self.close()
        self.observer = self.connect()
        self.observer.run("TRUNCATE freezes, receipts, batches, heads, revisions, domains; "
                          f"INSERT INTO domains(scope) VALUES ({SCOPE});")
        check(self.observer.value(submit(SEED)), expected(1, "x0", "y0"))

    def close(self):
        errors = []
        for session in self.sessions:
            try:
                session.close()
            except Exception as error:
                errors.append(str(error))
        self.sessions.clear()
        if errors:
            raise RuntimeError("session cleanup failed: " + "; ".join(errors))

    def state(self, scope=SCOPE) -> dict:
        # 单条 SELECT 的快照；完整集合用于失败前后比较，报告不记录正文。
        return self.observer.value(f"""
            SELECT jsonb_build_object(
                'domain', (SELECT to_jsonb(d) FROM domains d WHERE scope={scope}),
                'heads', (SELECT coalesce(jsonb_agg(to_jsonb(h) ORDER BY object_id),'[]')
                    FROM heads h WHERE scope={scope}),
                'revisions', (SELECT coalesce(jsonb_agg(to_jsonb(v) ORDER BY revision),'[]')
                    FROM revisions v WHERE scope={scope}),
                'batches', (SELECT coalesce(jsonb_agg(to_jsonb(b) ORDER BY restore,position),'[]')
                    FROM batches b WHERE scope={scope}),
                'receipts', (SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY restore,operation),'[]')
                    FROM receipts r WHERE scope={scope}),
                'freezes', (SELECT coalesce(jsonb_agg(to_jsonb(f) ORDER BY restore,identity),'[]')
                    FROM freezes f WHERE scope={scope}));
        """)

    def assert_business(self, high: int, heads: dict, operations: list[str]):
        state = self.state()
        check(state["domain"]["high_water"], high)
        check({h["object_id"]: (h["revision"], h["deleted"]) for h in state["heads"]}, heads)
        check([b["position"] for b in state["batches"]], list(range(1, high + 1)))
        check([b["body"]["id"] for b in state["batches"]], operations)
        for batch in state["batches"]:
            receipt = next(r for r in state["receipts"] if r["operation"] == batch["body"]["id"])
            check(receipt["body"], batch["body"])
            check(receipt["result"]["position"], batch["position"])

    def rejected(self, sql: str, reason: str):
        session = self.connect()
        try:
            session.run(sql)
        except RuntimeError as error:
            if "ERROR:  " + reason not in str(error):
                raise
        else:
            raise AssertionError("expected SQL rejection: " + reason)
        finally:
            session.close()

    def blocked(self, waiter: Session, blocker: Session):
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            if self.observer.value(
                f"SELECT to_jsonb({blocker.pid}=ANY(pg_blocking_pids({waiter.pid})));"
            ):
                self.report["barriers"].append({"waiter": waiter.pid, "blocker": blocker.pid})
                return
            time.sleep(0.02)  # 只退避查询；通过条件必须是 PostgreSQL 确认的阻塞关系。
        raise TimeoutError("expected PostgreSQL lock blocker not observed")

    def gate(self) -> Session:
        gate = self.connect()
        gate.run("SELECT pg_advisory_lock(701);")
        return gate

    def release(self, gate: Session):
        gate.run("SELECT pg_advisory_unlock(701);")

    def kill_backend(self, session: Session):
        check(self.observer.value(f"SELECT to_jsonb(pg_terminate_backend({session.pid}));"), True)

    def case(self, name: str, callback):
        self.reset()
        callback()
        state = self.state()
        self.report["cases"].append({
            "id": name, "passed": True,
            "highWater": state["domain"]["high_water"],
            "heads": {h["object_id"]: h["revision"] for h in state["heads"]},
            "batches": len(state["batches"]), "receipts": len(state["receipts"]),
        })
        print("PASS " + name, flush=True)
