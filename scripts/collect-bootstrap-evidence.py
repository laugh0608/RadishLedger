#!/usr/bin/env python3
"""只重组已保存的合成诊断分片；不连接设备，不判定业务或真机验收通过。"""
import argparse
import base64
import binascii
import json
from pathlib import Path
import re
import stat

ROOT = Path(__file__).resolve().parent.parent
MAX_INPUT = 2 * 1024 * 1024
MAX_PAYLOAD = 96 * 1024
CHUNK = 700
IDENTITY = re.compile(r"[0-9a-f]{32}")


def reject(message):
    raise ValueError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            reject("报告含重复 JSON 键")
        result[key] = value
    return result


def collect(text, session, report_id):
    if not IDENTITY.fullmatch(session) or not IDENTITY.fullmatch(report_id):
        reject("批次与报告身份必须为 32 位小写十六进制")
    if len(text.encode("utf-8")) > MAX_INPUT:
        reject("输入超过 2 MiB；只保留本批 App 的合成诊断")
    marker = f"RLM0_BOOTSTRAP:{session}:{report_id}:"
    chunks, count = {}, None
    for line in text.splitlines():
        if marker not in line:
            continue
        if line.count(marker) != 1:
            reject("同一行含重复报告标记")
        match = re.fullmatch(r"([0-9]{1,3}):([0-9]{1,3}):([A-Za-z0-9+/=]+)",
                             line.split(marker, 1)[1].strip())
        if match is None:
            reject("目标报告分片格式无效")
        index, total, data = match.groups()
        index, total = int(index), int(total)
        if not (0 <= index < total <= 188 and 0 < len(data) <= CHUNK):
            reject("目标报告分片越界")
        if index != total - 1 and len(data) != CHUNK:
            reject("非末尾分片被截断")
        if count is not None and count != total:
            reject("报告分片总数冲突")
        if index in chunks and chunks[index] != data:
            reject("同一分片内容冲突")
        count = total
        chunks[index] = data
    if count is None or len(chunks) != count:
        reject("未取得全部目标分片，不生成结果")
    try:
        payload = base64.b64decode("".join(chunks[i] for i in range(count)), validate=True)
        if len(payload) > MAX_PAYLOAD:
            reject("报告超过 96 KiB")
        report = json.loads(payload.decode("utf-8"), object_pairs_hook=unique_object,
                            parse_constant=lambda _: reject("报告含非标准 JSON 数值"),
                            parse_float=lambda _: reject("诊断报告数值必须为整数"))
    except (UnicodeError, json.JSONDecodeError, binascii.Error) as error:
        raise ValueError("报告编码或 JSON 无效") from error
    except RecursionError as error:
        raise ValueError("报告结构超过解析深度限制") from error
    if not isinstance(report, dict):
        reject("报告必须为 JSON 对象")
    if (type(report.get("schema")) is not int or report["schema"] != 1
            or report.get("session") != session or report.get("reportId") != report_id
            or report.get("scope") != "synthetic-bootstrap-observations"
            or report.get("hardwareVerified") is not False
            or report.get("acceptance") != "not-assessed"):
        reject("报告身份、版本或证据边界不符")
    observations, omitted = report.get("observations"), report.get("omittedObservations")
    if (not isinstance(observations, list) or len(observations) > 64
            or type(omitted) is not int or omitted < 0):
        reject("操作记录边界无效")
    # 这里只检查重组与报告边界；不从 succeeded、计数或摘要推断矩阵通过。
    return report


def collect_file(source, destination, session, report_id):
    evidence = (ROOT / "local-evidence/m0-c").resolve()
    source, destination = source.resolve(), destination.resolve()
    if not source.is_relative_to(evidence) or not destination.is_relative_to(evidence):
        reject("输入与输出必须位于本仓库 local-evidence/m0-c")
    attributes = source.stat()
    if not stat.S_ISREG(attributes.st_mode):
        reject("输入必须是已保存的普通诊断文件")
    if attributes.st_size > MAX_INPUT:
        reject("输入超过 2 MiB；只保留本批 App 的合成诊断")
    if destination.exists():
        reject("输出已存在，不能覆盖既有证据")
    try:
        with source.open("rb") as input_file:
            payload = input_file.read(MAX_INPUT + 1)
        if len(payload) > MAX_INPUT:
            reject("读取时输入超过 2 MiB，停止重组")
        text = payload.decode("utf-8")
    except UnicodeError as error:
        raise ValueError("输入不是有效 UTF-8 文本") from error
    report = collect(text, session, report_id)
    # exclusive create：失败或旧报告不覆盖，父目录需由调用者明确准备。
    with destination.open("x", encoding="utf-8", newline="\n") as output:
        output.write(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    return len(report["observations"]), report["omittedObservations"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--session", required=True)
    parser.add_argument("--report-id", required=True)
    args = parser.parse_args()
    try:
        count, omitted = collect_file(args.input, args.output, args.session, args.report_id)
    except ValueError as error:
        # ValueError 均为上方固定诊断，不回显输入内容。
        print(f"重组失败：{error}；未执行验收。")
        return 1
    except OSError as error:
        print(f"重组发生文件 IO 错误（errno={error.errno}）；保留现场，未执行验收。")
        return 1
    print(f"已重组 {count} 条观测，省略 {omitted} 条；仅完成传输检查，未执行验收。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
