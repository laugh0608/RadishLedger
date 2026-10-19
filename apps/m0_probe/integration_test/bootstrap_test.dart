import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:radishledger_probe/bootstrap_app.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/probe.dart';

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets(
    'actual bootstrap UI unlock failure cancel and retry',
    (tester) async {
      final platform = SystemProtection();
      final root = await platform.storage();
      final random = Random.secure();
      String identity() => List.generate(
        16,
        (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
      ).join();
      final reports = <String, Object?>{'platform': Platform.operatingSystem};
      var converted = false;
      Future<void> scroll(Finder finder, double delta) =>
          tester.scrollUntilVisible(
            finder,
            delta,
            scrollable: find.byType(Scrollable).first,
          );
      for (final bridge in BridgeCandidate.values) {
        final service = BootstrapService(root, identity(), bridge, platform);
        final password = Uint8List.fromList(
          utf8.encode(identity() + identity()),
        );
        final issued = await service.core({
          'kind': 'newRecovery',
        }, Uint8List(0));
        issued.requireSuccess();
        Future<BootstrapResult> call(
          Map<String, Object?> action,
          List<int> bytes,
        ) async {
          final result = await service.core(action, Uint8List.fromList(bytes));
          result.requireSuccess();
          return result;
        }

        Future<void> waitIdle() async {
          final deadline = DateTime.now().add(const Duration(seconds: 30));
          while (DateTime.now().isBefore(deadline)) {
            await tester.pump(const Duration(milliseconds: 50));
            if (tester
                    .widget<FilledButton>(
                      find.byKey(const ValueKey('m0c-unlock-password')),
                    )
                    .onPressed !=
                null) {
              return;
            }
          }
          fail('UI operation exceeded bounded deadline');
        }

        var created = false;
        try {
          (await call(
            {'kind': 'create'},
            [...password, ...issued.secret],
          )).clear();
          created = true;
          (await call({'kind': 'confirm'}, issued.secret)).clear();
          final seeded = await call({
            'kind': 'seed',
            'unlock': 'recovery',
          }, issued.secret);
          final digest = seeded.report['contentDigest'];
          final audit = seeded.report['audit'] as Map;
          final snapshot = audit['snapshot'] as Map;
          expect(audit['operations'], hasLength(7));
          expect(audit['revisions'], 8);
          expect(snapshot['deletedEventIds'], hasLength(1));
          expect(snapshot['unresolvedCandidateRevisionIds'], hasLength(2));
          expect(snapshot['netAssetsMinor'], '18000');
          seeded.clear();
          await tester.pumpWidget(BootstrapApp(service: service));
          await tester.pumpAndSettle();
          final field = find.byKey(const ValueKey('m0c-password'));
          final unlock = find.byKey(const ValueKey('m0c-unlock-password'));
          await tester.enterText(field, 'incorrect synthetic input');
          await tester.ensureVisible(unlock);
          await tester.tap(unlock);
          await waitIdle();
          await scroll(find.byKey(const ValueKey('m0c-error')), 180);
          expect(find.textContaining('解锁材料不正确'), findsOneWidget);
          expect(find.byKey(const ValueKey('m0c-success')), findsNothing);
          await scroll(field, -180);
          await tester.tap(field);
          await tester.pumpAndSettle();
          await tester.enterText(field, utf8.decode(password));
          expect(
            tester.widget<TextField>(field).controller!.text.length,
            password.length,
            reason: 'entered length',
          );
          await tester.ensureVisible(unlock);
          expect(
            tester.widget<TextField>(field).controller!.text.length,
            password.length,
            reason: 'length before tap',
          );
          await tester.tap(unlock);
          await tester.pump();
          await tester.tap(find.byTooltip('锁定'));
          await waitIdle();
          expect(find.byKey(const ValueKey('m0c-success')), findsNothing);
          await scroll(field, -180);
          expect(tester.widget<TextField>(field).controller!.text, isEmpty);
          await scroll(field, -180);
          await tester.tap(field);
          await tester.pumpAndSettle();
          await tester.enterText(field, utf8.decode(password));
          expect(
            tester.widget<TextField>(field).controller!.text.length,
            password.length,
            reason: 'entered length',
          );
          await tester.ensureVisible(unlock);
          expect(
            tester.widget<TextField>(field).controller!.text.length,
            password.length,
            reason: 'length before tap',
          );
          await tester.tap(unlock);
          await waitIdle();
          await scroll(find.byKey(const ValueKey('m0c-status')), 180);
          final visibleStatus = tester
              .widget<Text>(find.byKey(const ValueKey('m0c-status')))
              .data;
          final errors = find.byKey(const ValueKey('m0c-error'));
          expect(
            find.byKey(const ValueKey('m0c-success')),
            findsOneWidget,
            reason:
                'status=$visibleStatus; error=${errors.evaluate().isEmpty ? null : tester.widget<Text>(errors).data}',
          );
          await scroll(find.byKey(const ValueKey('m0c-success')), 180);
          expect(find.byKey(const ValueKey('m0c-success')), findsOneWidget);
          final reopened = await call({
            'kind': 'inspect',
            'unlock': 'recovery',
          }, issued.secret);
          expect(reopened.report['contentDigest'], digest);
          expect(reopened.report['audit'], audit);
          reopened.clear();
          await tester.ensureVisible(find.byKey(const ValueKey('m0c-success')));
          await tester.pumpAndSettle();
          if (Platform.isAndroid && !converted) {
            await binding.convertFlutterSurfaceToImage();
            converted = true;
            await tester.pumpAndSettle();
          }
          await binding.takeScreenshot(
            '${Platform.operatingSystem}-${bridge.name}-bootstrap',
          );
          reports[bridge.name] = {
            'fixture': 'A11',
            'nonemptyFactsPreserved': true,
            'wrongPasswordVisible': true,
            'cancelLateResultSuppressed': true,
            'retryUnlockedOriginal': true,
          };
        } finally {
          await tester.pumpWidget(const SizedBox.shrink());
          password.fillRange(0, password.length, 0);
          issued.clear();
          if (created) (await call({'kind': 'cleanup'}, [])).clear();
        }
      }
      binding.reportData ??= <String, dynamic>{};
      binding.reportData!.addAll(reports);
    },
    timeout: const Timeout(Duration(minutes: 5)),
  );
}
