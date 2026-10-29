import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/bootstrap_evidence.dart';
import 'package:radishledger_probe/src/probe.dart';
import 'package:radishledger_probe/src/probe_report.dart';

const session = '11111111111111111111111111111111';
const run = '22222222222222222222222222222222';

void main() {
  test(
    'lock review projection rejects extra fields and stays within report bound',
    () {
      const secret = 'synthetic-sensitive-extra';
      final evidence = BootstrapEvidence(session: session);
      for (var i = 0; i < BootstrapEvidence.maxObservations; i++) {
        evidence
            .begin(BootstrapAction.lockVerify, run, BridgeCandidate.frb)
            .failure(
              const BootstrapFailure('lock_review_mismatch'),
              report: {
                'lockReview': {
                  'reviewId': run,
                  'vault': run,
                  'device': run,
                  'reviewState': 'mismatch',
                  'fileOutcome': 'observed',
                  'baselineDigest': List.filled(64, 'a').join(),
                  'afterDigest': List.filled(64, 'b').join(),
                  'dbCoverage': 'observed',
                  'walCoverage': 'absentAtStart',
                  'shmCoverage': secret,
                  'lockedAttempted': true,
                  'accessDeniedObserved': false,
                  'coveredFilesDenied': true,
                  'coveredFilesRestored': true,
                  'accessRestored': false,
                  'resourcesClosed': true,
                  'controlWriteDeniedObserved': true,
                  'controlWriteRestored': true,
                  'sequence': 8,
                  'revisions': 8,
                  'operationCount': 7,
                  'candidateCount': 2,
                  'deletedCount': 1,
                  'secret': secret,
                  'audit': secret,
                  'nativeSample': {'path': secret},
                  'hardwareProtectionVerified': true,
                },
              },
            );
      }
      final text = evidence.snapshot().text;
      expect(text, isNot(contains(secret)));
      expect(utf8.encode(text).length, lessThanOrEqualTo(96 * 1024));
      final summary = jsonDecode(
        text,
      )['observations'][0]['summary']['lockReview'];
      expect(summary['reviewState'], 'mismatch');
      expect(summary.containsKey('hardwareProtectionVerified'), isFalse);
      expect(summary.containsKey('shmCoverage'), isFalse);
    },
  );

  test(
    'projection never exports extra secrets, raw errors, paths, or full audit',
    () async {
      const secret = 'never-export-this-synthetic-secret';
      final evidence = BootstrapEvidence(session: session);
      evidence.capabilities({
        'supported': false,
        'deviceSecure': false,
        'api': 35,
        'platformCode': 11,
        'environment': secret,
        'secret': secret,
        'hardwareClaim': 'TEE',
      });
      evidence
          .begin(BootstrapAction.unlockRecovery, run, BridgeCandidate.thin)
          .success({
            'secret': secret,
            'path': '/private/$secret',
            'pid': 123,
            'vault': run,
            'device': secret,
            'contentDigest': List.filled(64, 'a').join(),
            'fixtureComplete': true,
            'ready': true,
            'audit': {
              'sequence': 8,
              'revisions': 8,
              'operations': [secret],
              'snapshot': {
                'deletedEventIds': [secret],
                'unresolvedCandidateRevisionIds': [secret, secret],
                'balancesMinor': secret,
              },
            },
            'platformProtection': {
              'securityLevelCode': '1',
              'secret': secret,
              'hardwareVerified': true,
            },
          });
      evidence
          .begin(BootstrapAction.unlockSystem, run, BridgeCandidate.thin)
          .failure(const BootstrapFailure(secret, secret));
      evidence
          .begin(BootstrapAction.unlockSystem, run, BridgeCandidate.thin)
          .failure(const BootstrapFailure('user_cancelled', '10'));
      final snapshot = evidence.snapshot();
      expect(snapshot.text, isNot(contains(secret)));
      final data = jsonDecode(snapshot.text) as Map;
      expect(data['hardwareVerified'], isFalse);
      expect(data['acceptance'], 'not-assessed');
      expect(data['capabilities'], {
        'supported': false,
        'deviceSecure': false,
        'api': 35,
        'platformCode': 11,
      });
      final records = data['observations'] as List;
      expect(records.first['summary'], {
        'vault': run,
        'contentDigest': List.filled(64, 'a').join(),
        'fixtureComplete': true,
        'ready': true,
        'pid': 123,
        'sequence': 8,
        'revisions': 8,
        'operationCount': 1,
        'deletedCount': 1,
        'candidateCount': 2,
        'securityLevelCode': '1',
      });
      expect(records[1]['errorCode'], 'unrecognized_error');
      expect(records[1].containsKey('platformCode'), isFalse);
      expect(records[2]['platformCode'], 10);
      final lines = <String>[];
      await snapshot.publish(lines.add);
      expect(lines.join(), isNot(contains(secret)));
      expect(lines, isNotEmpty);
    },
  );

  test(
    'bounded history distinguishes stopped waiting, late success, unknown and local cancel',
    () {
      final evidence = BootstrapEvidence(session: session);
      for (var i = 0; i < 67; i++) {
        evidence
            .begin(BootstrapAction.unlockPassword, run, BridgeCandidate.frb)
            .success({});
      }
      final late = evidence.begin(
        BootstrapAction.seed,
        run,
        BridgeCandidate.frb,
      )..stopWaiting();
      late.success({'fixtureComplete': true});
      evidence.begin(BootstrapAction.cleanup, run, BridgeCandidate.frb).success(
        {'actionCancelled': true},
      );
      evidence.begin(BootstrapAction.lock, run, BridgeCandidate.frb).unknown();
      final data = jsonDecode(evidence.snapshot().text);
      expect(data['omittedObservations'], 6);
      expect(data['observations'], hasLength(64));
      expect(data['observations'][61]['outcome'], 'succeeded');
      expect(data['observations'][61]['waitStopped'], isTrue);
      expect(data['observations'][62]['outcome'], 'cancelledBeforeAction');
      expect(data['observations'][63]['outcome'], 'unknown');
      expect(
        () => late.failure(const BootstrapFailure('interrupted')),
        throwsStateError,
      );
      expect(
        utf8.encode(evidence.snapshot().text).length,
        lessThanOrEqualTo(probeReportMaxBytes),
      );
    },
  );

  test(
    'missing or invalid session never publishes and snapshot is immutable',
    () async {
      for (final session in ['', 'raw-secret-invalid-session']) {
        final evidence = BootstrapEvidence(session: session);
        final operation = evidence.begin(
          BootstrapAction.create,
          run,
          BridgeCandidate.thin,
        );
        final before = evidence.snapshot();
        operation.success({'ready': true});
        expect(
          jsonDecode(before.text)['observations'][0]['outcome'],
          'inProgress',
        );
        expect(before.text, isNot(contains('raw-secret')));
        final output = <String>[];
        await expectLater(before.publish(output.add), throwsStateError);
        expect(output, isEmpty);
      }
    },
  );

  test(
    'output failure stays visible and retry uses the same immutable report identity',
    () async {
      final evidence = BootstrapEvidence(session: session);
      for (var i = 0; i < 12; i++) {
        evidence
            .begin(BootstrapAction.unlockRecovery, run, BridgeCandidate.thin)
            .success({'ready': true});
      }
      final snapshot = evidence.snapshot();
      final partial = <String>[];
      await expectLater(
        snapshot.publish((line) {
          if (partial.length == 1) {
            throw StateError('synthetic transport failure');
          }
          partial.add(line);
        }),
        throwsStateError,
      );
      expect(partial, hasLength(1));
      final retry = <String>[];
      await snapshot.publish(retry.add);
      expect(retry.length, greaterThan(1));
      expect(retry.first, partial.first);
      expect(evidence.snapshot().reportId, isNot(snapshot.reportId));
    },
  );

  test(
    'existing App file framing stays compatible and malformed chunks never emit',
    () async {
      final bytes = utf8.encode(
        jsonEncode({
          'session': session,
          'synthetic': List.filled(1800, '字').join(),
        }),
      );
      final chunks = probeReportChunks(bytes);
      final lines = <String>[];
      await emitProbeReport('RLM0_APP_BOUNDARY:$session', chunks, lines.add);
      expect(
        lines.first,
        startsWith('RLM0_APP_BOUNDARY:$session:0:${chunks.length}:'),
      );
      expect(
        base64Decode(lines.map((line) => line.split(':').last).join()),
        bytes,
      );
      expect(
        () => probeReportChunks(List.filled(probeReportMaxBytes + 1, 0)),
        throwsStateError,
      );
      for (final malformed in [
        <String>[],
        ['injected\ntext'],
        ['YQ==', 'Yg=='],
      ]) {
        final output = <String>[];
        await expectLater(
          emitProbeReport('RLM0_APP_BOUNDARY:$session', malformed, output.add),
          throwsStateError,
        );
        expect(output, isEmpty);
      }
      await expectLater(
        emitProbeReport('invalid', chunks, lines.add),
        throwsStateError,
      );
      await expectLater(
        emitProbeReport(
          'RLM0_APP_BOUNDARY:$session',
          chunks,
          (_) => throw StateError('synthetic output failure'),
        ),
        throwsStateError,
      );
    },
  );

  test(
    'Dart snapshot round-trips through the actual offline Python collector',
    () async {
      final evidence = BootstrapEvidence(session: session);
      evidence
          .begin(BootstrapAction.unlockSystem, run, BridgeCandidate.thin)
          .failure(const BootstrapFailure('item_missing'));
      final snapshot = evidence.snapshot();
      final lines = <String>[];
      await snapshot.publish(lines.add);
      final root = Directory.current.parent.parent;
      final parent = await Directory(
        '${root.path}/local-evidence/m0-c',
      ).create(recursive: true);
      final directory = await parent.createTemp('bootstrap-collector-test-');
      final input = File('${directory.path}/filtered.log');
      final output = File('${directory.path}/report.json');
      try {
        // 重复的同一分片可来自显式重试；乱序和非目标批次不应污染快照。
        await input.writeAsString(
          [
            'unrelated synthetic line',
            ...lines.reversed,
            lines.first,
          ].join('\n'),
        );
        final result = await Process.run('python3', [
          '-B',
          '${root.path}/scripts/collect-bootstrap-evidence.py',
          '--input',
          input.path,
          '--output',
          output.path,
          '--session',
          session,
          '--report-id',
          snapshot.reportId,
        ]);
        expect(result.exitCode, 0, reason: '${result.stdout}${result.stderr}');
        expect(
          jsonDecode(await output.readAsString()),
          jsonDecode(snapshot.text),
        );
        expect(result.stdout, contains('未执行验收'));
      } finally {
        if (await input.exists()) await input.delete();
        if (await output.exists()) await output.delete();
        await directory.delete();
      }
    },
  );
}
