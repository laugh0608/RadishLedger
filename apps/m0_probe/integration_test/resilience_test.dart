import 'dart:async';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:radishledger_probe/probe_app.dart';
import 'package:radishledger_probe/src/probe.dart';
import 'package:radishledger_probe/src/validation.dart';

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets(
    'actual bridge pressure, faults, concurrency and UI cancellation',
    (tester) async {
      final reports = <String, Object?>{};
      for (final candidate in BridgeCandidate.values) {
        reports[candidate.name] = await runBridgePressure(candidate);
        var calls = 0;
        var beats = 0;
        ProbeReport? actual;
        Object? failure;
        var finished = false;
        final timer = Timer.periodic(
          const Duration(milliseconds: 10),
          (_) => beats++,
        );
        await tester.pumpWidget(
          ProbeApp(
            run: (_) async {
              calls++;
              try {
                actual = await runProbe(candidate);
                return actual!;
              } catch (error) {
                failure = error;
                rethrow;
              } finally {
                finished = true;
              }
            },
          ),
        );
        try {
          await tester.tap(find.byKey(const ValueKey('run-probe')));
          await tester.pump();
          await tester.tap(find.byKey(const ValueKey('run-probe')));
          await tester.tap(find.byKey(const ValueKey('stop-waiting')));
          await tester.pump();
          expect(calls, 1);
          expect(find.textContaining('核心仍在完成'), findsOneWidget);
          final deadline = DateTime.now().add(const Duration(minutes: 2));
          while (!finished && DateTime.now().isBefore(deadline)) {
            await tester.pump(const Duration(milliseconds: 50));
          }
          expect(failure, isNull);
          expect(finished, isTrue);
          expect(actual!.checkpointCount, 40);
          expect(beats, greaterThan(0));
          await tester.pumpAndSettle();
          expect(find.byKey(const ValueKey('probe-success')), findsNothing);
          expect(find.text('当前实验已结束，可以重新运行。'), findsOneWidget);
          reports['${candidate.name}Cancellation'] = {
            'invocations': calls,
            'heartbeatTicks': beats,
            'coreCheckpoints': actual!.checkpointCount,
            'lateResultHidden': true,
          };
        } finally {
          timer.cancel();
        }
        await tester.pumpWidget(const SizedBox.shrink());
      }
      binding.reportData = {
        'schema': 1,
        'platform': Platform.operatingSystem,
        'resilience': reports,
      };
    },
    timeout: const Timeout(Duration(minutes: 10)),
  );
}
