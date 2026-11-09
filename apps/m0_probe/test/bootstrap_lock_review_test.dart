import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/bootstrap_evidence.dart';
import 'package:radishledger_probe/src/bootstrap_lock_review.dart';
import 'package:radishledger_probe/src/bootstrap_selection.dart';
import 'package:radishledger_probe/src/probe.dart';
import 'bootstrap_experiment_test.dart' show MemoryProtection, failure;

// 仅平台文件状态为替身；真实两桥接 / SQLCipher 测试另行标明。
class FileObservationProtection extends MemoryProtection {
  String outcome = 'lockedIoNotExecuted';
  Map<String, String> coverage = {
    'ledger.db': 'observed',
    'ledger.db-wal': 'absentAtStart',
    'ledger.db-shm': 'absentAtStart',
  };
  Completer<Map<String, dynamic>>? pendingObservation;
  String? observedRun;
  String? observedId;
  Map<String, dynamic> fileReport(String run, String id) => {
    'schema': 1,
    'run': run,
    'observation': id,
    'outcome': outcome,
    'lockedAttempted': false,
    'hardwareProtectionVerified': false,
    'acceptance': 'not-assessed',
    'coverage': coverage,
    'resources': {'writeControlCloseErrno': 0, 'directoryCloseErrno': 0},
    'unexpectedSecret': 'never-retain-native-extras',
  };
  @override
  Future<Map<String, dynamic>> observeBootstrapFiles(
    String run,
    String observation,
  ) async {
    observedRun = run;
    observedId = observation;
    return pendingObservation == null
        ? fileReport(run, observation)
        : pendingObservation!.future;
  }

  @override
  Future<void> cancelBootstrapObservation(
    String run,
    String observation,
  ) async {
    if (run == observedRun && observation == observedId) {
      outcome = 'cancelled';
      pendingObservation?.complete(fileReport(run, observation));
    }
  }
}

Map<String, dynamic> syntheticInspect() => {
  'vault': '11111111111111111111111111111111',
  'device': '22222222222222222222222222222222',
  'ready': true,
  'fixtureComplete': true,
  'contentDigest': List.filled(64, 'a').join(),
  'audit': {
    'sequence': 8,
    'revisions': 8,
    'operations': List.filled(7, 'synthetic'),
    'snapshot': {
      'deletedEventIds': ['synthetic'],
      'unresolvedCandidateRevisionIds': ['synthetic-a', 'synthetic-b'],
      'netAssetsMinor': '18000',
    },
  },
};

class InspectService extends BootstrapService {
  InspectService(FileObservationProtection platform)
    : super(
        '/synthetic',
        '33333333333333333333333333333333',
        BridgeCandidate.thin,
        platform,
      );
  Map<String, dynamic> data = syntheticInspect();
  BootstrapFailure? fail;
  final actions = <String>[];
  @override
  Future<BootstrapResult> core(
    Map<String, Object?> action,
    Uint8List input, {
    String? pauseAt,
    Map<String, String>? fileFault,
  }) async {
    input.fillRange(0, input.length, 0);
    actions.add(action['kind'] as String);
    if (fail != null) throw fail!;
    return BootstrapResult({'ok': true, 'report': data}, Uint8List(0));
  }
}

void main() {
  for (final bridge in BridgeCandidate.values) {
    test(
      '${bridge.name} real core same-vault review, wrong material, cancellation and cleanup retry',
      () async {
        final directory = await Directory.systemTemp.createTemp(
          'radishledger_lock_review_',
        );
        const run = '55555555555555555555555555555555';
        final platform = FileObservationProtection();
        final service = BootstrapService(directory.path, run, bridge, platform);
        final password = Uint8List.fromList(
          utf8.encode('synthetic-lock-review-password'),
        );
        final generated = await service.core({
          'kind': 'newRecovery',
        }, Uint8List(0));
        generated.requireSuccess();
        final recovery = Uint8List.fromList(generated.secret);
        generated.clear();
        Future<void> core(Map<String, Object?> action, List<int> input) async {
          final result = await service.core(action, Uint8List.fromList(input));
          try {
            result.requireSuccess();
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
          await expectLater(
            BootstrapLockReview.prepare(
              service,
              'recovery',
              Uint8List.fromList(recovery),
            ),
            throwsA(failure('lock_baseline_invalid')),
          );
          await core({'kind': 'seed', 'unlock': 'recovery'}, recovery);
          final input = Uint8List.fromList(password);
          final review = await BootstrapLockReview.prepare(
            service,
            'password',
            input,
          );
          expect(input, everyElement(0));
          expect(review.baseline['operationCount'], 7);
          expect(review.baseline['candidateCount'], 2);
          expect(review.baseline['deletedCount'], 1);
          expect(review.baseline.keys, isNot(contains('audit')));
          for (final name in platform.coverage.keys) {
            platform.coverage[name] =
                await File('${directory.path}/m0c-$run/$name').exists()
                ? 'observed'
                : 'absentAtStart';
          }
          final observed = await review.observe();
          expect(review.fileOutcome.name, 'lockedIoNotExecuted');
          expect(
            observed.report['lockReview']['reviewState'],
            'awaitingUnlock',
          );
          observed.clear();
          await service.lock();
          await expectLater(
            review.verify(
              service,
              'password',
              Uint8List.fromList(utf8.encode('wrong synthetic input')),
              isCurrent: () => true,
            ),
            throwsA(failure('authentication_failed')),
          );
          expect(review.state.name, 'unlockFailed');
          final verified = await review.verify(
            service,
            'recovery',
            Uint8List.fromList(recovery),
            isCurrent: () => true,
          );
          expect(review.state.name, 'matched');
          expect(
            review.fileOutcome.name,
            'lockedIoNotExecuted',
          ); // SQL 成功不补记锁屏拒绝。
          final evidence = BootstrapEvidence();
          evidence
              .begin(BootstrapAction.lockVerify, run, bridge)
              .success(verified.report);
          verified.clear();
          final report = jsonDecode(
            evidence.snapshot().text,
          )['observations'][0]['summary']['lockReview'];
          expect(report['baselineDigest'], report['afterDigest']);
          expect(report['walCoverage'], platform.coverage['ledger.db-wal']);
          expect(report.containsKey('accessDeniedObserved'), isFalse);
          expect(
            evidence.snapshot().text,
            isNot(contains('never-retain-native-extras')),
          );

          (await service.establishProtection(
            'recovery',
            Uint8List.fromList(recovery),
          )).clear();
          final systemReview = await BootstrapLockReview.prepare(
            service,
            'system',
            Uint8List(0),
          );
          expect(systemReview.baseline, review.baseline);
          platform.pendingObservation = Completer<Map<String, dynamic>>();
          final waiting = systemReview.observe();
          await service.lock(); // 后台只取消认证，不能取消文件观察。
          expect(systemReview.observing, isTrue);
          await systemReview.cancelObservation();
          (await waiting).clear();
          expect(systemReview.fileOutcome.name, 'cancelled');
          (await systemReview.verify(
            service,
            'system',
            Uint8List(0),
            isCurrent: () => true,
          )).clear();
          expect(systemReview.state.name, 'matched');
          await expectLater(
            systemReview.verify(
              service,
              'recovery',
              Uint8List.fromList(recovery),
              isCurrent: () => false,
            ),
            throwsA(failure('interrupted')),
          );
          expect(systemReview.state.name, 'interrupted');
          (await systemReview.verify(
            service,
            'recovery',
            Uint8List.fromList(recovery),
            isCurrent: () => true,
          )).clear();
          final generation = await service.activeProtection();
          platform.rejectDelete = generation;
          await expectLater(
            service.cleanupSelection(),
            throwsA(failure('temporarily_unavailable')),
          );
          expect(
            await Directory('${directory.path}/m0c-$run').exists(),
            isTrue,
          );
          platform.rejectDelete = null;
          (await service.cleanupSelection()).clear();
          expect(await directory.list().isEmpty, isTrue);
        } finally {
          password.fillRange(0, password.length, 0);
          recovery.fillRange(0, recovery.length, 0);
          for (final key in platform.keys.values) {
            key.fillRange(0, key.length, 0);
          }
          if (await directory.list().isEmpty) await directory.delete();
        }
      },
    );
  }

  test(
    'mismatch, identity change and malformed A11 stop without seed or repair',
    () async {
      for (final field in [
        'contentDigest',
        'vault',
        'device',
        'audit',
        'ready',
      ]) {
        final platform = FileObservationProtection();
        final service = InspectService(platform);
        final review = await BootstrapLockReview.prepare(
          service,
          'password',
          Uint8List(1),
        );
        (await review.observe()).clear();
        service.data = syntheticInspect();
        service.data[field] = switch (field) {
          'contentDigest' => List.filled(64, 'b').join(),
          'vault' || 'device' => '99999999999999999999999999999999',
          'audit' => {'sequence': 8},
          _ => false,
        };
        await expectLater(
          review.verify(
            service,
            'password',
            Uint8List(1),
            isCurrent: () => true,
          ),
          throwsA(failure('lock_review_mismatch')),
        );
        expect(review.state.name, 'mismatch');
        expect(service.actions, ['inspect', 'inspect']);
        expect(review.baseline['contentDigest'], List.filled(64, 'a').join());
      }
    },
  );

  test(
    'foreign service, stale completion, resource failure and wrong native identity do not pass',
    () async {
      final platform = FileObservationProtection();
      final service = InspectService(platform);
      final review = await BootstrapLockReview.prepare(
        service,
        'password',
        Uint8List(1),
      );
      platform.pendingObservation = Completer<Map<String, dynamic>>();
      final waiting = review.observe();
      platform.pendingObservation!.complete({
        ...platform.fileReport(service.run, review.id),
        'observation': service.run,
      });
      await expectLater(waiting, throwsA(failure('invalid_response')));
      expect(review.fileOutcome.name, 'failed');
      final input = Uint8List.fromList([42]);
      await expectLater(
        review.verify(
          InspectService(platform),
          'password',
          input,
          isCurrent: () => true,
        ),
        throwsA(failure('selection_changed')),
      );
      expect(input, [0]);
      expect(service.actions, ['inspect']);
      await expectLater(
        review.verify(
          service,
          'password',
          Uint8List(1),
          isCurrent: () => false,
        ),
        throwsA(failure('interrupted')),
      );
      expect(review.state.name, 'interrupted');
      expect(review.report['lockReview']['afterDigest'], isNull);
      platform.pendingObservation = Completer<Map<String, dynamic>>();
      final second = await BootstrapLockReview.prepare(
        service,
        'password',
        Uint8List(1),
      );
      final secondWaiting = second.observe();
      platform.pendingObservation!.complete({
        ...platform.fileReport(service.run, second.id),
        'resources': {'writeControlCloseErrno': 5, 'directoryCloseErrno': 0},
      });
      await expectLater(
        secondWaiting,
        throwsA(failure('lock_resources_unconfirmed')),
      );
      expect(second.report['lockReview']['resourcesClosed'], isFalse);
      expect(second.fileOutcome.name, 'failed');
    },
  );
}
