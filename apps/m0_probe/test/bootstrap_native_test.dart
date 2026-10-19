import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/probe.dart';

void main() {
  for (final bridge in BridgeCandidate.values) {
    test(
      '${bridge.name} preserves nonempty facts through publication faults',
      () async {
        final directory = Directory.systemTemp.createTempSync(
          'radishledger_m0c_',
        );
        final random = Random.secure();
        final run = List.generate(
          16,
          (_) => random.nextInt(256),
        ).map((n) => n.toRadixString(16).padLeft(2, '0')).join();
        final secrets = <Uint8List>[];
        Uint8List keep(List<int> value) {
          final bytes = Uint8List.fromList(value);
          secrets.add(bytes);
          return bytes;
        }

        Future<BootstrapResult> invoke(
          Map<String, Object?> action,
          Uint8List secret, {
          Map<String, String>? fault,
        }) => callBootstrap(
          bridge,
          directory.path,
          run,
          action,
          Uint8List.fromList(secret),
          fileFault: fault,
        );

        Future<Map<String, dynamic>> success(
          Map<String, Object?> action,
          Uint8List secret,
        ) async {
          final result = await invoke(action, secret);
          try {
            result.requireSuccess();
            return result.report;
          } finally {
            result.clear();
          }
        }

        final empty = Uint8List(0);
        var created = false;
        try {
          final generated = await invoke({'kind': 'newRecovery'}, empty);
          generated.requireSuccess();
          final recovery = keep(generated.secret);
          generated.clear();
          final password = keep(
            utf8.encode(
              List.generate(
                32,
                (_) => random.nextInt(256),
              ).map((n) => n.toRadixString(16).padLeft(2, '0')).join(),
            ),
          );
          await success({'kind': 'create'}, keep([...password, ...recovery]));
          created = true;
          await success({'kind': 'confirm'}, recovery);
          final baseline = await success({
            'kind': 'seed',
            'unlock': 'recovery',
          }, recovery);
          final audit = baseline['audit'] as Map;
          final snapshot = audit['snapshot'] as Map;
          expect(audit['sequence'], 8);
          expect(audit['revisions'], 8);
          expect(audit['operations'], hasLength(7));
          expect(snapshot['deletedEventIds'], hasLength(1));
          expect(snapshot['unresolvedCandidateRevisionIds'], hasLength(2));
          expect(snapshot['netAssetsMinor'], '18000');

          void preserved(Map<String, dynamic> report) {
            for (final field in [
              'vault',
              'device',
              'contentDigest',
              'audit',
              'ready',
              'fixtureComplete',
            ]) {
              expect(report[field], baseline[field], reason: field);
            }
          }

          for (final stage in [
            'shortWrite',
            'writeZero',
            'writeError',
            'fileSync',
            'publish',
            'directorySync',
          ]) {
            final prepared = await invoke({
              'kind': 'prepareProtection',
              'unlock': 'recovery',
            }, recovery);
            prepared.requireSuccess();
            final generation = prepared.report['candidateProtection'] as String;
            final local = keep(prepared.secret);
            prepared.clear();
            final result = await invoke(
              {'kind': 'commitProtection', 'generation': generation},
              local,
              fault: {'file': 'activeProtection', 'stage': stage},
            );
            try {
              expect(result.status['ok'], stage == 'shortWrite');
              if (stage != 'shortWrite') {
                expect(
                  (result.status['error'] as Map)['code'],
                  stage == 'directorySync'
                      ? 'bootstrap_publish_uncertain'
                      : 'bootstrap_io',
                );
              }
            } finally {
              result.clear();
            }
            preserved(
              await success({
                'kind': 'inspect',
                'unlock': 'recovery',
              }, recovery),
            );
            await success({
              'kind': 'commitProtection',
              'generation': generation,
            }, local);
            preserved(
              await success({'kind': 'inspect', 'unlock': 'system'}, local),
            );
          }
          preserved(
            await success({'kind': 'replay', 'unlock': 'password'}, password),
          );
        } finally {
          for (final secret in secrets) {
            secret.fillRange(0, secret.length, 0);
          }
          if (created) await success({'kind': 'cleanup'}, empty);
          expect(directory.listSync(), isEmpty);
          directory.deleteSync();
        }
      },
    );
  }
}
