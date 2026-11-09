#!/usr/bin/env python3
"""在已安装的专用 App 入口核对安全域 / 文件属性，支持一次实际进程中断与恢复清理。"""
import argparse
import base64
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import time

from mobile_probe_device import Device, run

ROOT = Path(__file__).resolve().parent.parent


def validate(report, platform, session, interrupted):
    assert report['schema'] == 1 and report['session'] == session and report['platform'] == platform
    assert report['passed'] and report['cleaned'] and not report['cleanupErrors'], report.get('error')
    assert report['resumedRuns'] == ([interrupted['run']] if interrupted else [])
    env = report['environment']
    assert env['pid'] == report['pid']
    if interrupted:
        assert report['pid'] != interrupted['pid']
    assert env['storageMetadata']['mode'] == 0o700
    if platform == 'android':
        assert report['release'] and not env['debuggable'] and not env['allowBackup']
        assert env['selinux'].startswith('u:r:untrusted_app')
        assert env['storage'] == env['noBackup'] + '/m0-c'
    else:
        assert env['environment'] == 'simulator' and env['storageMetadata']['ownExcludedFromBackup']
        assert env['storage'] == env['home'] + '/Library/Application Support/m0-c'
        assert report['fileProtectionEvidence'] == 'unavailableOnSimulator'
        assert env['storageMetadata']['fileProtection'] is None
    assert report['hardwareProtectionVerified'] is False
    assert len(report['cases']) == 4
    assert {(c['bridge'], c['core']['fileControl']) for c in report['cases']} == {
        ('thin', False), ('thin', True), ('frb', False), ('frb', True)}
    configuration = report['cases'][0]['core']['configuration']
    assert configuration['os'] == platform and configuration['arch'] == 'aarch64'
    assert configuration['defaultVfs'] == 'unix'
    for case in report['cases']:
        core, attributes = case['core'], case['attributes']
        assert core['configuration'] == configuration
        assert core['pid'] == report['pid'] and core['defaultVfsUnchanged'] and core['fullFactsPreserved']
        root = env['storage'] + '/m0c-' + case['run']
        assert core['directory'] == root and attributes['root']['path'] == root
        assert attributes['root']['mode'] == 0o700 and attributes['root']['uid'] == env['uid']
        files = attributes['files']
        assert {PurePosixPath(f['path']).name for f in files} == {
            'owner', 'lock', 'ledger.db', 'ledger.db-wal', 'ledger.db-shm', 'auxiliary.db', 'scanner-control.db'}
        for file in files:
            assert str(PurePosixPath(file['path']).parent) == root
            assert file['mode'] == 0o600 and file['uid'] == env['uid']
            if platform == 'ios':
                assert file['fileProtection'] is None
            else:
                assert file['selinux']['errno'] == 0 and ':app_data_file:' in file['selinux']['value']
        sizes = {PurePosixPath(f['path']).name: f['bytes'] for f in files}
        assert sizes['ledger.db-wal'] == core['walBytesForPlatformAudit'] > 32
        assert sizes['ledger.db-shm'] == core['shmBytesForPlatformAudit'] > 0
        io = core['io']['files']
        assert core['ioEventCount'] > 0 and all(f['closed'] and f['scan_code'] == 0 for f in io)
        assert {'database', 'wal', 'journal', 'superJournal'} <= {f['kind'] for f in io}
        for file in io:
            if file['path'] is not None:
                assert str(PurePosixPath(file['path']).parent) == root
                assert file['owner_uid'] == env['uid']
                assert file['mode'] in ({0o600, 0o644} if file['kind'] == 'superJournal' else {0o600})
        temporary = [f for f in io if f['kind'] in {'tempDatabase', 'tempJournal', 'subJournal', 'transient'}]
        forced = 'TEMP_STORE=3' in configuration['compileOptions']
        assert core['forcedMemory'] == forced
        if not core['fileControl'] or forced:
            assert not temporary and all(not f['disk_contains_marker'] and not f['write_contains_marker'] for f in io)
        else:
            assert {f['kind'] for f in temporary} == {'tempDatabase', 'tempJournal', 'subJournal', 'transient'}
            assert any(f['anonymous'] and f['disk_contains_marker'] for f in temporary)
        assert core['scannerControl']['disk_contains_marker'] and core['scannerControl']['scanned_bytes'] > 0
        audit = core['a11Audit']; snapshot = audit['snapshot']
        assert audit['sequence'] == 8 and audit['revisions'] == 8 and len(audit['operations']) == 7
        assert len(snapshot['unresolvedCandidateRevisionIds']) == 2 and len(snapshot['deletedEventIds']) == 1
        assert snapshot['netAssetsMinor'] == '18000' and sorted(map(int, snapshot['balancesMinor'].values())) == [8000, 10000]


def android_output(device, pid, session):
    logs = device.shell('logcat', '-d', '--pid=' + str(pid), '-v', 'brief', '-s', 'flutter:I').stdout
    chunks = {}
    count = None
    for index, total, data in re.findall(r'RLM0_APP_BOUNDARY:' + session + r':(\d+):(\d+):([A-Za-z0-9+/=]+)', logs):
        index, total = int(index), int(total)
        assert 0 <= index < total <= 188 and 0 < len(data) <= 700
        assert index == total - 1 or len(data) == 700
        assert count is None or count == total
        count = total
        assert index not in chunks or chunks[index] == data
        chunks[index] = data
    report = None
    if count and len(chunks) == count:
        payload = base64.b64decode(''.join(chunks[i] for i in range(count)), validate=True)
        assert len(payload) <= 96 * 1024
        report = json.loads(payload)
    ready = re.findall(r'RLM0_APP_BOUNDARY_READY:' + session + r':([0-9a-f]{32})', logs)
    return report, ready[-1] if ready else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=['android', 'ios'], required=True)
    parser.add_argument('--device', required=True)
    parser.add_argument('--session', required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--interrupt-once', action='store_true')
    args = parser.parse_args()
    assert re.fullmatch('[0-9a-f]{32}', args.session)
    output = args.output_dir.resolve()
    assert output.is_relative_to(ROOT / 'local-evidence/m0-c')
    output.mkdir(parents=True, exist_ok=True)
    report_path = output / f'{args.platform}-app-files.json'
    assert not report_path.exists(), '不能覆盖既有证据'
    device = Device(args.platform, args.device)
    report_name = f'm0-app-boundary-{args.session}.json'
    journal_name = f'm0-app-boundary-owned-{args.session}.json'
    if args.platform == 'ios':
        assert device.read(report_name) is None, '同一报告已存在，不能混入旧批次'
    interrupted = None
    try:
        device.launch()
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            if args.platform == 'android':
                found = device.shell('pidof', device.package, check=False).stdout.strip()
                if not found:
                    raise RuntimeError('App 进程已退出，不能当作验证通过')
                pid = int(found)
                report, ready = android_output(device, pid, args.session)
            else:
                raw = device.read(report_name)
                try:
                    report = json.loads(raw) if raw else None
                except json.JSONDecodeError:
                    report = None  # App 写入尚未完成；仍受整体超时约束。
                raw = device.read(journal_name)
                journal = json.loads(raw) if raw else None
                ready = (journal['owned'][0]['run'] if journal and journal['phase'] == 'filesReady' else None)
                pid = journal['pid'] if journal else None
            if args.interrupt_once and interrupted is None and ready:
                interrupted = {'pid': pid, 'run': ready, 'method': 'force-stop' if args.platform == 'android' else 'SIGKILL'}
                (output / f'{args.platform}-interruption.json').write_text(json.dumps(interrupted, indent=2) + '\n')
                if args.platform == 'ios':
                    device.kill(pid)
                else:
                    device.stop()
                    assert not device.shell('pidof', device.package, check=False).stdout.strip()
                device.launch()
                continue
            if report is not None:
                report_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')
                if args.platform == 'ios':
                    device.remove_result(report_name)
                assert not args.interrupt_once or interrupted is not None, report.get('error')
                validate(report, args.platform, args.session, interrupted)
                if args.platform == 'android':
                    denied = device.shell('run-as', device.package, 'id', check=False)
                    assert denied.returncode != 0 and 'not debuggable' in (denied.stdout + denied.stderr)
                    access = device.shell('ls', '-ld', report['environment']['storage'], check=False)
                    assert access.returncode != 0 and 'Permission denied' in (access.stdout + access.stderr)
                    (output / 'android-shell-denial.json').write_text(json.dumps({
                        'runAsExit': denied.returncode, 'runAs': denied.stdout + denied.stderr,
                        'shellAccessExit': access.returncode, 'shellAccess': access.stdout + access.stderr}, indent=2) + '\n')
                    with (output / 'android-app-files.png').open('wb') as image:
                        subprocess.run([device.adb, '-s', args.device, 'exec-out', 'screencap', '-p'], stdout=image, check=True)
                else:
                    assert device.read(journal_name) is None
                    for run_id in [c['run'] for c in report['cases']] + report['resumedRuns']:
                        assert not Path(report['environment']['storage'], 'm0c-' + run_id).exists()
                    time.sleep(0.8)  # 等专用入口发布有界诊断后呈现最终状态；截图另行视觉复核。
                    run(['xcrun', 'simctl', 'io', args.device, 'screenshot', str(output / 'ios-app-files.png')])
                print(f'{args.platform}: 4 App cases passed, interruption={bool(interrupted)}, cleaned=true')
                return
            time.sleep(0.15)
        raise RuntimeError('等待 App 合成报告超时；检查本批登记，不能清理其他目录')
    finally:
        device.stop()


if __name__ == '__main__':
    main()
