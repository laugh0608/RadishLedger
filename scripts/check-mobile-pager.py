#!/usr/bin/env python3
"""在既有模拟器的 App 私有目录运行 cfg(test) 原生 pager；不安装或启动设备。"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import uuid

from mobile_probe_device import Device

ROOT = Path(__file__).resolve().parent.parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=['ios', 'android'], required=True)
    parser.add_argument('--device', required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--configuration-only', action='store_true')
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output_dir.resolve()
    if not output.is_relative_to(ROOT / 'local-evidence/m0-c'):
        raise RuntimeError('证据必须在本仓库忽略目录 local-evidence/m0-c 内')
    output.mkdir(parents=True, exist_ok=True)
    summary_path = output / f'{args.platform}-run.json'
    if summary_path.exists():
        raise RuntimeError('不覆盖既有批次，请指定新的 output-dir')
    device = Device(args.platform, args.device)
    if args.platform == 'android':
        identity = dict(uid=device.shell('run-as', device.package, 'id').stdout.strip(),
                        selinux=device.shell('run-as', device.package, 'id', '-Z').stdout.strip(),
                        sdk=device.shell('getprop', 'ro.build.version.sdk').stdout.strip(),
                        pageBytes=device.shell('getconf', 'PAGESIZE').stdout.strip())
    else:
        inventory = json.loads(subprocess.check_output(
            ['xcrun', 'simctl', 'list', 'devices', '--json'], text=True))
        identity = next(dict(runtime=runtime, name=d['name'])
                        for runtime, devices in inventory['devices'].items()
                        for d in devices if d['udid'] == args.device)
    name = 'm0-pager-' + uuid.uuid4().hex
    relative = 'code_cache/' + name if args.platform == 'android' else None
    if args.platform == 'android':
        base = device.shell('run-as', device.package, 'pwd').stdout.strip()
        directory = base + '/' + relative
    else:
        directory = str(device.container / 'tmp' / name)
    executable = directory + '/pager-tests'
    environment = dict(TMPDIR=directory, SQLITE_TMPDIR=directory,
                       M0_PAGER_EVIDENCE_DIR=directory + '/evidence')
    summary = dict(platform=args.platform, device=args.device, directory=directory, identity=identity,
                   binarySha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                   scope='native simulator test process in project App data directory; not Flutter or device security',
                   cleaned=False, runs=[])
    summary_path.write_text(json.dumps(summary, indent=2) + '\n')
    tests = [('target_configuration', ['configuration.json'])]
    if not args.configuration_only:
        tests.extend([
            ('pager_partial_writes_and_sync_errors_recover_atomic_facts', ['pager-report.json']),
            ('temporary_files_observed_before_unlink_and_memory_policy_checked', ['temporary-files-report.json'])])
    try:
        if args.platform == 'android':
            device.shell('run-as', device.package, 'mkdir', relative)
            device.shell('run-as', device.package, 'mkdir', relative + '/evidence')
            with binary.open('rb') as source:
                subprocess.run([device.adb, '-s', args.device, 'shell', 'run-as', device.package,
                                'sh', '-c', shlex.quote('cat > ' + relative + '/pager-tests')],
                               stdin=source, check=True, timeout=60)
            device.shell('run-as', device.package, 'chmod', '700', relative + '/pager-tests')
        else:
            Path(directory).mkdir()
            Path(directory, 'evidence').mkdir()
            shutil.copy2(binary, Path(directory, 'pager-tests'))
        for test, reports in tests:
            arguments = ['--exact', 'store::pager_tests::' + test, '--nocapture', '--test-threads=1']
            if args.platform == 'android':
                command = [device.adb, '-s', args.device, 'shell', 'run-as', device.package, 'env',
                           *[f'{k}={v}' for k, v in environment.items()], executable, *arguments]
                env = None
            else:
                command = ['xcrun', 'simctl', 'spawn', args.device, executable, *arguments]
                env = dict(os.environ, **{'SIMCTL_CHILD_' + k: v for k, v in environment.items()})
            log = output / f'{args.platform}-{test}.log'
            with log.open('w') as stream:
                result = subprocess.run(command, env=env, stdout=stream, stderr=subprocess.STDOUT)
            summary['runs'].append(dict(test=test, exitCode=result.returncode))
            summary_path.write_text(json.dumps(summary, indent=2) + '\n')
            if result.returncode:
                raise RuntimeError(f'{test} 失败，见 {log}；保留精确目录以便排查')
            for report in reports:
                if args.platform == 'android':
                    text = device.shell('run-as', device.package, 'cat', directory + '/evidence/' + report).stdout
                else:
                    text = Path(directory, 'evidence', report).read_text()
                parsed = json.loads(text)
                if report == 'configuration.json':
                    if parsed['os'] != args.platform or parsed['arch'] != 'aarch64':
                        raise RuntimeError('实际测试目标不符，不能使用宿主库或其他 ABI')
                    configuration = parsed
                else:
                    if not (parsed['allPassed'] and parsed['cleaned']):
                        raise RuntimeError('测试未证实通过及原始文件清理')
                    workers = (parsed['controls'] + [c['worker'] for c in parsed['cases']]
                               if report == 'pager-report.json' else [c['worker'] for c in parsed['cases']])
                    if any(w['configuration'] != configuration for w in workers):
                        raise RuntimeError('工作进程 VFS / 编译配置与预检不一致')
                (output / f'{args.platform}-{report}').write_text(text)
            print(f'{args.platform}: {test} passed', flush=True)
        # 只删除本次 UUID 独占目录；失败时保留并在摘要记录，不扩大清理范围。
        if args.platform == 'android':
            entries = device.shell('run-as', device.package, 'ls', '-A', directory).stdout.splitlines()
        else:
            entries = [p.name for p in Path(directory).iterdir()]
        if sorted(entries) != ['evidence', 'pager-tests']:
            raise RuntimeError(f'测试目录有未清理原始数据：{entries}')
        if args.platform == 'android':
            device.shell('run-as', device.package, 'rm', '-r', directory)
            assert device.shell('run-as', device.package, 'test', '-e', directory, check=False).returncode == 1
        else:
            assert Path(directory).parent == device.container / 'tmp'
            shutil.rmtree(directory)
            assert not Path(directory).exists()
        summary['cleaned'] = True
    finally:
        summary_path.write_text(json.dumps(summary, indent=2) + '\n')


if __name__ == '__main__':
    main()
