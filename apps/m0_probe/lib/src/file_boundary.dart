// M0 合成文件观测；本入口不接受用户账库或解锁材料。
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'package:flutter/foundation.dart';
import 'bootstrap.dart';
import 'native.dart';
import 'probe.dart';
import 'probe_report.dart';
import 'validation.dart';

void _require(bool condition, String reason) {
  if (!condition) throw StateError(reason);
}

String fileProtectionEvidence(Map<String, dynamic> environment, Map metadata) {
  if (metadata['fileProtection'] == 'NSFileProtectionComplete') {
    return 'attributeObservedHardwareUnverified';
  }
  _require(
    environment['environment'] == 'simulator' &&
        metadata['fileProtection'] == null,
    'iOS 文件保护属性不符',
  );
  return 'unavailableOnSimulator';
}

void verifyAppEnvironment(Map<String, dynamic> environment) {
  final storage = environment['storage'] as String;
  final metadata = environment['storageMetadata'] as Map;
  _require(
    metadata['path'] == storage &&
        metadata['mode'] == 448 &&
        metadata['uid'] == environment['uid'],
    '私有根目录路径、权限或属主不符',
  );
  if (environment['platform'] == 'android') {
    _require(
      environment['debuggable'] == false && environment['allowBackup'] == false,
      'Android 必须实际关闭调试和自动备份',
    );
    _require(
      (environment['selinux'] as String).startsWith('u:r:untrusted_app'),
      'Android 必须由正常 App 安全域运行',
    );
    _require(
      storage == '${environment['noBackup']}/m0-c',
      '账库没有位于 noBackup 目录内',
    );
  } else {
    _require(environment['platform'] == 'ios', '目标平台不支持');
    _require(metadata['ownExcludedFromBackup'] == true, 'iOS 私有根没有排除自动备份');
    _require(
      storage == '${environment['home']}/Library/Application Support/m0-c',
      'iOS 账库不在预期 App 容器',
    );
    fileProtectionEvidence(environment, metadata);
  }
}

void verifyAppFileAttributes(
  Map<String, dynamic> environment,
  Map<String, dynamic> attributes,
  String run,
) {
  final expected = '${environment['storage']}/m0c-$run';
  final root = attributes['root'] as Map;
  _require(
    root['path'] == expected &&
        root['mode'] == 448 &&
        root['uid'] == environment['uid'],
    '本次账库目录属性不符',
  );
  final files = List<Map>.from(attributes['files'] as List);
  const names = {
    'owner',
    'lock',
    'ledger.db',
    'ledger.db-wal',
    'ledger.db-shm',
    'auxiliary.db',
    'scanner-control.db',
  };
  _require(
    files.length == names.length &&
        files
            .map((f) => (f['path'] as String).split('/').last)
            .toSet()
            .containsAll(names),
    '实际文件类别不完整',
  );
  for (final file in files) {
    _require(
      (file['path'] as String).startsWith('$expected/') &&
          file['mode'] == 384 &&
          file['uid'] == environment['uid'],
      '文件越界、权限或属主不符',
    );
    if (environment['platform'] == 'ios') {
      fileProtectionEvidence(environment, file);
    } else {
      final label = file['selinux'] as Map;
      _require(
        label['errno'] == 0 &&
            (label['value'] as String).contains(':app_data_file:'),
        'Android 实际文件安全标签不符',
      );
    }
  }
  if (environment['platform'] == 'ios') {
    fileProtectionEvidence(environment, root);
  }
}

String _id() {
  final random = Random.secure();
  return List.generate(
    16,
    (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
}

Future<Map<String, dynamic>> _platform(
  String method, [
  Map<String, String>? args,
]) async {
  final value = await SystemProtection.channel.invokeMapMethod<String, dynamic>(
    method,
    args,
  );
  if (value == null) throw StateError('平台未返回文件观测');
  return value;
}

Future<void> _saveOwned(
  File journal,
  String session,
  String storage,
  List<(BridgeCandidate, String)> owned,
  String phase,
) async {
  final pending = File('${journal.path}.pending');
  await pending.writeAsString(
    jsonEncode({
      'session': session,
      'storage': storage,
      'pid': pid,
      'phase': phase,
      'owned': owned
          .map((entry) => {'bridge': entry.$1.name, 'run': entry.$2})
          .toList(),
    }),
    flush: true,
  );
  await pending.rename(journal.path);
}

Future<void> _removeOwned(
  String storage,
  List<(BridgeCandidate, String)> owned,
) async {
  for (final entry in List<(BridgeCandidate, String)>.from(owned)) {
    final directory = Directory('$storage/m0c-${entry.$2}');
    if (await directory.exists()) {
      final result = await callValidation(entry.$1, {
        'kind': 'fileBoundaryCleanup',
        'run': entry.$2,
      }, directory: storage);
      _require(result['ok'] == true && !await directory.exists(), '本次材料未精确清理');
    }
    owned.remove(entry);
  }
}

Future<Map<String, dynamic>> runAppFileBoundary(
  String session, {
  bool observeIosLock = false,
  void Function(bool waiting)? onLockObservation,
}) async {
  final report = <String, dynamic>{
    'schema': 1,
    'session': session,
    'pid': pid,
    'platform': Platform.operatingSystem,
    'release': kReleaseMode,
    'cases': <Object?>[],
    'passed': false,
    'cleaned': false,
  };
  final owned = <(BridgeCandidate, String)>[];
  String? storage;
  File? journal;
  var journalValid = true;
  try {
    _require(RegExp(r'^[0-9a-f]{32}$').hasMatch(session), '必须提供唯一的合成运行身份');
    _require(!observeIosLock || Platform.isIOS, '锁屏观测仅支持 iOS 专用入口');
    journal = File(
      '${Directory.systemTemp.path}/m0-app-boundary-owned-$session.json',
    );
    journalValid = !await journal.exists();
    final environment = await _platform('fileEnvironment');
    report['environment'] = environment;
    report['dartTemporary'] = Directory.systemTemp.path;
    report['loadedLibrary'] = loadedLibraryPath();
    verifyAppEnvironment(environment);
    report['fileProtectionEvidence'] = Platform.isIOS
        ? fileProtectionEvidence(
            environment,
            environment['storageMetadata'] as Map,
          )
        : 'notApplicable';
    report['hardwareProtectionVerified'] = false;
    _require(environment['pid'] == pid, '平台调用不在同一 App 进程');
    storage = environment['storage'] as String;
    if (await journal.exists()) {
      _require(await journal.length() <= 16384, '本次登记超过上限');
      final saved = jsonDecode(await journal.readAsString()) as Map;
      _require(
        saved['session'] == session &&
            saved['storage'] == storage &&
            (saved['owned'] as List).length <= 4,
        '本次登记身份或路径不符',
      );
      for (final item in saved['owned'] as List) {
        final run = item['run'] as String;
        _require(
          RegExp(r'^[0-9a-f]{32}$').hasMatch(run) &&
              !owned.any((entry) => entry.$2 == run),
          '登记包含无效身份',
        );
        owned.add((
          BridgeCandidate.values.byName(item['bridge'] as String),
          run,
        ));
      }
      journalValid = true;
      report['resumedRuns'] = owned.map((entry) => entry.$2).toList();
      await _removeOwned(storage, owned);
      await _saveOwned(journal, session, storage, owned, 'resumed');
    } else {
      report['resumedRuns'] = <String>[];
    }
    for (final bridge in BridgeCandidate.values) {
      for (final fileControl in [false, true]) {
        final run = _id();
        owned.add((bridge, run));
        await _saveOwned(journal, session, storage, owned, 'running');
        final value = await callValidation(bridge, {
          'kind': 'fileBoundary',
          'run': run,
          'fileControl': fileControl,
        }, directory: storage);
        _require(value['ok'] == true, '核心文件观测失败：${value['error']}');
        final core = Map<String, dynamic>.from(value['report'] as Map);
        await _saveOwned(journal, session, storage, owned, 'filesReady');
        if ((report['cases'] as List).isEmpty) {
          debugPrintSynchronously('RLM0_APP_BOUNDARY_READY:$session:$run');
          await Future<void>.delayed(const Duration(seconds: 3));
        }
        final attributes = await _platform('fileAttributes', {'run': run});
        (report['cases'] as List).add({
          'bridge': bridge.name,
          'run': run,
          'core': core,
          'attributes': attributes,
        });
        _require(
          core['pid'] == pid &&
              core['configuration']['os'] == Platform.operatingSystem &&
              core['defaultVfsUnchanged'] == true &&
              core['fullFactsPreserved'] == true,
          '核心进程、VFS 或账务保全不符',
        );
        verifyAppFileAttributes(environment, attributes, run);
        if (observeIosLock && (report['cases'] as List).length == 1) {
          onLockObservation?.call(true);
          try {
            report['lockObservation'] = {
              'bridge': bridge.name,
              'run': run,
              ...await _platform('observeFileProtection', {'run': run}),
            };
          } finally {
            onLockObservation?.call(false);
          }
        }
        final audit = core['a11Audit'] as Map;
        final snapshot = audit['snapshot'] as Map;
        _require(
          audit['sequence'] == 8 &&
              audit['revisions'] == 8 &&
              (audit['operations'] as List).length == 7 &&
              (snapshot['unresolvedCandidateRevisionIds'] as List).length ==
                  2 &&
              (snapshot['deletedEventIds'] as List).length == 1 &&
              snapshot['netAssetsMinor'] == '18000',
          'A11 独立计数不符',
        );
        final cleanup = await callValidation(bridge, {
          'kind': 'fileBoundaryCleanup',
          'run': run,
        }, directory: storage);
        _require(
          cleanup['ok'] == true && cleanup['report']['cleaned'] == true,
          '核心没有证实精确清理',
        );
        _require(!await Directory('$storage/m0c-$run').exists(), '本次账库目录仍残留');
        owned.removeLast();
        await _saveOwned(journal, session, storage, owned, 'cleaned');
      }
    }
    report['passed'] = true;
  } catch (error) {
    report['error'] = error.toString();
  } finally {
    final errors = <String>[];
    if (!journalValid) errors.add('登记无法核对，保留现场');
    if (storage != null && journalValid) {
      try {
        await _removeOwned(storage, owned);
      } catch (error) {
        errors.add(error.toString());
      }
      if (journal != null) {
        try {
          if (owned.isEmpty) {
            if (await journal.exists()) await journal.delete();
            final pending = File('${journal.path}.pending');
            if (await pending.exists()) await pending.delete();
          } else {
            await _saveOwned(journal, session, storage, owned, 'cleanupFailed');
          }
        } catch (error) {
          errors.add(error.toString());
        }
      }
    }
    report['cleanupErrors'] = errors;
    report['cleaned'] = errors.isEmpty;
    if (errors.isNotEmpty) report['passed'] = false;
  }
  return report;
}

Future<void> cancelFileProtectionObservation() => SystemProtection.channel
    .invokeMethod<void>('cancelFileProtectionObservation');

/// Android release 不开放 run-as 或导出组件；仅此专用入口输出有界合成诊断。
Future<void> publishAppBoundaryReport(Map<String, dynamic> report) async {
  final bytes = utf8.encode(jsonEncode(report));
  final chunks = probeReportChunks(bytes);
  final session = report['session'] as String;
  if (Platform.isIOS) {
    await File(
      '${Directory.systemTemp.path}/m0-app-boundary-$session.json',
    ).writeAsBytes(bytes, flush: true);
  }
  await emitProbeReport(
    'RLM0_APP_BOUNDARY:$session',
    chunks,
    debugPrintSynchronously,
  );
}
