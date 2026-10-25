import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/bootstrap_selection.dart';
import 'package:radishledger_probe/src/probe.dart';

// 只替代平台认证 / 密钥存储。两 bridge 以下均调用实际 Rust / SQLCipher。
class MemoryProtection extends SystemProtection {
  final keys = <String, Uint8List>{};
  final deleted = <String>[];
  String? rejectDelete;
  bool rejectCreate = false;
  Completer<void>? deleteStarted;
  Completer<void>? continueDelete;
  @override
  Future<Map<String, dynamic>> capabilities() async => {'supported': true};
  @override
  Future<void> cancel() async {}
  @override
  Future<Map<String, dynamic>> create(
    String run,
    String generation,
    Uint8List secret,
  ) async {
    try {
      if (rejectCreate) throw const BootstrapFailure('user_cancelled');
      keys['$run.$generation'] = Uint8List.fromList(secret);
      return {
        'created': true,
        'securityLevelCode': '1',
        'unexpectedSecret': 'must-not-be-retained',
      };
    } finally {
      secret.fillRange(0, secret.length, 0);
    }
  }

  @override
  Future<Uint8List> read(String run, String generation) async {
    final value = keys['$run.$generation'];
    if (value == null) throw const BootstrapFailure('item_missing');
    return Uint8List.fromList(value);
  }

  @override
  Future<Map<String, dynamic>> delete(String run, String generation) async {
    if (generation == rejectDelete) {
      throw const BootstrapFailure('temporarily_unavailable');
    }
    final started = deleteStarted;
    if (started != null && !started.isCompleted) {
      started.complete();
      await continueDelete!.future;
    }
    deleted.add('$run.$generation');
    final old = keys.remove('$run.$generation');
    old?.fillRange(0, old.length, 0);
    return {'deleted': true, 'alreadyMissing': old == null};
  }
}

Matcher failure(String code) =>
    isA<BootstrapFailure>().having((e) => e.code, 'code', code);

void main() {
  for (final bridge in BridgeCandidate.values) {
    test(
      '${bridge.name} exact item loss preserves A11 and cleanup retries recorded candidates',
      () async {
        final directory = await Directory.systemTemp.createTemp(
          'radishledger_experiment_',
        );
        final random = Random.secure();
        String identity() => List.generate(
          16,
          (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
        ).join();
        final run = identity();
        final platform = MemoryProtection();
        final service = BootstrapService(directory.path, run, bridge, platform);
        final password = Uint8List.fromList(
          utf8.encode(identity() + identity()),
        );
        final generated = await service.core({
          'kind': 'newRecovery',
        }, Uint8List(0));
        generated.requireSuccess();
        final recovery = Uint8List.fromList(generated.secret);
        generated.clear();
        final unrelated = await File(
          '${directory.path}/unrelated',
        ).writeAsString('synthetic retained');
        final otherKey = '${identity()}.${identity()}';
        platform.keys[otherKey] = Uint8List.fromList(List.filled(32, 37));
        Future<Map<String, dynamic>> core(
          Map<String, Object?> action,
          List<int> secret,
        ) async {
          final result = await service.core(action, Uint8List.fromList(secret));
          try {
            result.requireSuccess();
            return result.report;
          } finally {
            result.clear();
          }
        }

        try {
          await BootstrapSelection(
            run,
            bridge,
          ).save(directory.path, create: true);
          await core({'kind': 'create'}, [...password, ...recovery]);
          await core({'kind': 'confirm'}, recovery);
          final baseline = await core({
            'kind': 'seed',
            'unlock': 'recovery',
          }, recovery);
          final audit = baseline['audit'] as Map;
          expect(audit['sequence'], 8);
          expect(audit['revisions'], 8);
          expect(audit['operations'], hasLength(7));
          expect(audit['snapshot']['deletedEventIds'], hasLength(1));
          expect(
            audit['snapshot']['unresolvedCandidateRevisionIds'],
            hasLength(2),
          );
          expect(audit['snapshot']['netAssetsMinor'], '18000');
          void preserved(Map<String, dynamic> actual) {
            for (final field in [
              'vault',
              'device',
              'contentDigest',
              'audit',
              'ready',
              'fixtureComplete',
            ]) {
              expect(actual[field], baseline[field], reason: field);
            }
          }

          final established = await service.establishProtection(
            'recovery',
            Uint8List.fromList(recovery),
          );
          established.requireSuccess();
          preserved(established.report);
          expect(established.report['platformProtection'], {
            'securityLevelCode': '1',
            'hardwareVerified': false,
          });
          established.clear();
          final active = await service.activeProtection();
          await expectLater(
            service.removeActiveProtection(identity()),
            throwsA(failure('selection_changed')),
          );
          expect(platform.deleted, isEmpty);
          (await service.removeActiveProtection(active)).clear();
          await expectLater(
            service.unlockSystem(),
            throwsA(failure('item_missing')),
          );
          await expectLater(
            core({
              'kind': 'inspect',
              'unlock': 'password',
            }, utf8.encode('wrong synthetic password')),
            throwsA(failure('authentication_failed')),
          );
          preserved(
            await core({'kind': 'inspect', 'unlock': 'recovery'}, recovery),
          );

          // 已产生核心候选但平台认证取消；清理必须覆盖这一未活动候选。
          platform.rejectCreate = true;
          await expectLater(
            service.establishProtection(
              'recovery',
              Uint8List.fromList(recovery),
            ),
            throwsA(failure('user_cancelled')),
          );
          platform.rejectCreate = false;
          (await service.establishProtection(
            'recovery',
            Uint8List.fromList(recovery),
          )).clear();
          final reopened = await BootstrapService(
            directory.path,
            run,
            bridge,
            platform,
          ).unlockSystem();
          preserved(reopened.report);
          reopened.clear();
          final status = await core({'kind': 'status'}, []);
          final candidates = List<String>.from(status['candidates']);
          expect(candidates, hasLength(3));

          platform.rejectDelete = candidates.first;
          await expectLater(
            service.cleanupSelection(),
            throwsA(failure('temporarily_unavailable')),
          );
          expect(
            (await BootstrapSelection.load(directory.path))!.cleanupCandidates,
            candidates,
          );
          preserved(
            await core({'kind': 'inspect', 'unlock': 'recovery'}, recovery),
          );
          platform.rejectDelete = null;
          platform.deleteStarted = Completer<void>();
          platform.continueDelete = Completer<void>();
          final interruptedCleanup = service.cleanupSelection();
          await platform.deleteStarted!.future;
          await service.lock();
          final interrupted = expectLater(
            interruptedCleanup,
            throwsA(failure('interrupted')),
          );
          platform.continueDelete!.complete();
          await interrupted;
          preserved(
            await core({'kind': 'inspect', 'unlock': 'recovery'}, recovery),
          );
          expect(
            (await BootstrapSelection.load(directory.path))!.cleanupCandidates,
            candidates,
          );
          final cleaned = await BootstrapService(
            directory.path,
            run,
            bridge,
            platform,
          ).cleanupSelection();
          expect(cleaned.report['cleaned'], isTrue);
          expect(cleaned.report['removedProtectionCount'], 3);
          cleaned.clear();
          expect(await BootstrapSelection.load(directory.path), isNull);
          expect(
            await Directory('${directory.path}/m0c-$run').exists(),
            isFalse,
          );
          for (final generation in candidates) {
            expect(platform.deleted, contains('$run.$generation'));
          }
          expect(platform.keys.keys, [otherKey]);
          expect(await unrelated.readAsString(), 'synthetic retained');

          final next = BootstrapSelection(
            identity(),
            BridgeCandidate.values.firstWhere((b) => b != bridge),
          );
          await next.save(directory.path, create: true);
          expect(
            (await BootstrapSelection.load(directory.path))!.bridge,
            next.bridge,
          );
          await File('${directory.path}/selection.json').delete();
        } finally {
          password.fillRange(0, password.length, 0);
          recovery.fillRange(0, recovery.length, 0);
          for (final key in platform.keys.values) {
            key.fillRange(0, key.length, 0);
          }
          // 测试私有临时目录；失败时保留归属现场，不以递归删除掩盖残留。
          await unrelated.delete();
          if (await directory.list().isEmpty) await directory.delete();
        }
      },
    );
  }

  test(
    'cleanup resumes after core removal without status and refuses foreign selection',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'radishledger_cleanup_',
      );
      const run = '11111111111111111111111111111111';
      const generation = '22222222222222222222222222222222';
      final platform = MemoryProtection();
      final selection = BootstrapSelection(
        run,
        BridgeCandidate.thin,
        cleanupCandidates: [generation],
      );
      await selection.save(directory.path, create: true);
      final foreign = BootstrapService(
        directory.path,
        '33333333333333333333333333333333',
        BridgeCandidate.thin,
        platform,
      );
      await expectLater(
        foreign.cleanupSelection(),
        throwsA(failure('selection_changed')),
      );
      expect(platform.deleted, isEmpty);
      // 无核心目录，若重新调用 status 就会失败；必须从既有清理计划重试。
      final cleaned = await BootstrapService(
        directory.path,
        run,
        BridgeCandidate.thin,
        platform,
      ).cleanupSelection();
      expect(cleaned.report['cleaned'], isTrue);
      cleaned.clear();
      expect(platform.deleted, ['$run.$generation']);
      expect(await directory.list().isEmpty, isTrue);
      await directory.delete();
    },
  );

  test(
    'selection corruption, links and incomplete publication never become an empty vault',
    () async {
      final directory = await Directory.systemTemp.createTemp(
        'radishledger_selection_',
      );
      final file = File('${directory.path}/selection.json');
      final platform = MemoryProtection();
      const run = '11111111111111111111111111111111';
      final service = BootstrapService(
        directory.path,
        run,
        BridgeCandidate.thin,
        platform,
      );
      for (final content in [
        '{',
        jsonEncode({'run': '../escape', 'bridge': 'thin'}),
        jsonEncode({
          'run': run,
          'bridge': 'thin',
          'cleanupCandidates': [run, run],
        }),
      ]) {
        await file.writeAsString(content);
        await expectLater(
          service.cleanupSelection(),
          throwsA(failure('invalid_selection')),
        );
        expect(await file.readAsString(), content);
      }
      await file.delete();
      final target = await File(
        '${directory.path}/unrelated',
      ).writeAsString('retained');
      final link = await Link(file.path).create(target.path);
      await expectLater(
        service.cleanupSelection(),
        throwsA(failure('invalid_selection')),
      );
      await link.delete();
      final pending = await File('${file.path}.pending').writeAsString('{}');
      await expectLater(
        BootstrapSelection.load(directory.path),
        throwsA(failure('selection_publish_incomplete')),
      );
      expect(platform.deleted, isEmpty);
      expect(await target.readAsString(), 'retained');
      await pending.delete();
      await target.delete();
      await directory.delete();
    },
  );
}
