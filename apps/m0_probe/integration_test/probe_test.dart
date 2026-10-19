import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:radishledger_probe/probe_app.dart';
import 'package:radishledger_probe/src/probe.dart';

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets(
    'both bridges execute the actual packaged Rust core',
    (tester) async {
      final results = <String, Object?>{};
      var surfaceConverted = false;
      for (final candidate in BridgeCandidate.values) {
        ProbeReport? actual;
        BridgeCandidate? selected;
        await tester.pumpWidget(
          ProbeApp(
            run: (bridge) async {
              selected = bridge;
              actual = await runProbe(bridge);
              return actual!;
            },
          ),
        );
        if (candidate == BridgeCandidate.frb) {
          await tester.tap(find.byKey(const ValueKey('bridge-candidate')));
          await tester.pumpAndSettle();
          await tester.tap(find.text('flutter_rust_bridge').last);
          await tester.pumpAndSettle();
        }
        await tester.tap(find.byKey(const ValueKey('run-probe')));
        final deadline = DateTime.now().add(const Duration(minutes: 3));
        while (actual == null &&
            DateTime.now().isBefore(deadline) &&
            find.byKey(const ValueKey('probe-error')).evaluate().isEmpty) {
          await tester.pump(const Duration(milliseconds: 200));
        }
        expect(find.byKey(const ValueKey('probe-error')), findsNothing);
        expect(actual, isNotNull, reason: 'actual core response required');
        expect(selected, candidate);
        await tester.pumpAndSettle();
        expect(find.byKey(const ValueKey('probe-success')), findsOneWidget);
        expect(actual!.caseCount, 11);
        expect(actual!.checkpointCount, 40);
        results[candidate.name] = actual!.data;
        if (Platform.isAndroid && !surfaceConverted) {
          await binding.convertFlutterSurfaceToImage();
          surfaceConverted = true;
          await tester.pumpAndSettle();
        }
        await binding.takeScreenshot(
          '${Platform.operatingSystem}-${candidate.name}',
        );
        await tester.pumpWidget(const SizedBox.shrink());
      }
      expect(
        (results['thin']! as Map<String, Object?>)['cases'],
        (results['frb']! as Map<String, Object?>)['cases'],
        reason: 'same effective facts and amounts',
      );
      binding.reportData ??= <String, dynamic>{};
      binding.reportData!.addAll(results);
    },
    timeout: const Timeout(Duration(minutes: 10)),
  );
}
