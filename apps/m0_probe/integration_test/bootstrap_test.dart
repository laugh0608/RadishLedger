import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:radishledger_probe/bootstrap_app.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/bootstrap_selection.dart';
import 'package:radishledger_probe/src/probe.dart';

class LifecycleEvidence with WidgetsBindingObserver {
  final states = <AppLifecycleState>[];
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) => states.add(state);
}

void main() {
  final binding = IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets(
    'actual manual bootstrap lifecycle, review, diagnostics and cleanup',
    (tester) async {
      const session = String.fromEnvironment('M0C_EVIDENCE_SESSION');
      expect(
        RegExp(r'^[0-9a-f]{32}$').hasMatch(session),
        isTrue,
        reason: 'explicit evidence session required',
      );
      final platform = SystemProtection();
      final root = await platform.storage();
      // 已有 selection 一律停止，不用清理测试制造空基线。
      expect(
        await BootstrapSelection.load(root),
        isNull,
        reason: 'existing selection must be preserved',
      );
      final capability = await platform.capabilities();
      final reports = <String, dynamic>{
        'platform': Platform.operatingSystem,
        'session': session,
        'completed': false,
        'cleanupComplete': false,
        'ownedRuns': <String>[],
        'systemProtection': {
          'supported': capability['supported'] == true,
          'execution': 'notRunAuthenticationRequiresSeparateDeviceConditions',
        },
      };
      binding.reportData ??= <String, dynamic>{};
      binding.reportData!.addAll(reports);
      final marker = File('$root/ui-background-$session.json');
      final markerPending = File('${marker.path}.pending');
      expect(
        await marker.exists(),
        isFalse,
        reason: 'do not overwrite previous batch marker',
      );
      expect(
        await markerPending.exists(),
        isFalse,
        reason: 'incomplete previous marker must be preserved',
      );
      var converted = false;
      Future<void> settle() async {
        await tester.pumpAndSettle(
          const Duration(milliseconds: 100),
          EnginePhase.sendSemanticsUpdate,
          const Duration(seconds: 10),
        );
      }

      Future<void> reveal(Finder finder, {double step = 200}) async {
        for (var count = 0; count < 80; count++) {
          if (finder.evaluate().isNotEmpty) {
            await tester.ensureVisible(finder);
            await tester.pump(const Duration(milliseconds: 50));
            if (finder.hitTestable().evaluate().isNotEmpty) return;
          }
          await tester.drag(find.byType(ListView), Offset(0, -step));
          await tester.pump(const Duration(milliseconds: 50));
        }
        fail('bounded UI control reveal failed');
      }

      Future<void> tap(Finder finder, {double step = 200}) async {
        await reveal(finder, step: step);
        await tester.tap(finder);
        await tester.pump();
      }

      Future<void> waitUntil(bool Function() ready) async {
        final deadline = DateTime.now().add(const Duration(seconds: 40));
        while (DateTime.now().isBefore(deadline)) {
          await tester.pump(const Duration(milliseconds: 50));
          if (ready()) return;
        }
        fail('bounded UI state transition did not complete');
      }

      Future<void> idle() => waitUntil(
        () =>
            tester
                .widget<IconButton>(find.byKey(const ValueKey('m0c-evidence')))
                .onPressed !=
            null,
      );
      final passwordField = find.byKey(const ValueKey('m0c-password'));
      final recoveryField = find.byKey(const ValueKey('m0c-recovery'));
      Future<void> enter(Finder field, Uint8List bytes) async {
        await reveal(field, step: -200);
        await tester.tap(field);
        await tester.pump();
        await tester.enterText(field, utf8.decode(bytes));
      }

      Future<void> assertInputsCleared() async {
        await reveal(passwordField, step: -200);
        expect(
          tester.widget<TextField>(passwordField).controller!.text.isEmpty,
          isTrue,
        );
        expect(
          tester.widget<TextField>(recoveryField).controller!.text.isEmpty,
          isTrue,
        );
        expect(find.byKey(const ValueKey('m0c-issued')), findsNothing);
      }

      Future<void> screenshot(BridgeCandidate bridge, String phase) async {
        await assertInputsCleared();
        await reveal(
          find.byKey(
            ValueKey(
              phase == 'cleanup-retry' ? 'm0c-status' : 'm0c-review-state',
            ),
          ),
        );
        await settle();
        if (Platform.isAndroid && !converted) {
          await binding.convertFlutterSurfaceToImage();
          converted = true;
          await settle();
        }
        await binding.takeScreenshot(
          '${Platform.operatingSystem}-${bridge.name}-bootstrap-$phase',
        );
      }

      await tester.pumpWidget(const BootstrapApp());
      await waitUntil(
        () => tester.widget<TextField>(passwordField).enabled == true,
      );
      try {
        for (final bridge in BridgeCandidate.values) {
          void phase(String value) =>
              debugPrint('M0C UI ${bridge.name}: $value');
          phase('start');
          final random = Random.secure();
          final password = Uint8List.fromList(
            utf8.encode(
              List.generate(
                32,
                (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
              ).join(),
            ),
          );
          Uint8List? recovery;
          BootstrapService? service;
          Directory? obstacle;
          var cleaned = false;
          try {
            final dropdown = find.byType(
              DropdownButtonFormField<BridgeCandidate>,
            );
            if (bridge == BridgeCandidate.frb) {
              await tap(dropdown, step: -200);
              await settle();
              await tester.tap(find.text('frb').last);
              await settle();
            }
            expect(
              tester.state<FormFieldState<BridgeCandidate>>(dropdown).value,
              bridge,
            );
            phase('create');
            await enter(passwordField, password);
            await tap(find.byKey(const ValueKey('m0c-create')));
            await idle();
            final selection = await BootstrapSelection.load(root);
            expect(selection?.bridge, bridge);
            service = BootstrapService(root, selection!.run, bridge, platform);
            (reports['ownedRuns'] as List<String>).add(service.run);
            phase('read-issued');
            final issued = find.byKey(const ValueKey('m0c-issued'));
            await reveal(issued, step: -200);
            recovery = Uint8List.fromList(
              utf8.encode(tester.widget<Text>(issued).data!),
            );
            expect(recovery.length == 80, isTrue); // 不将材料放入断言失败文本。
            phase('confirm');
            await tap(find.text('隐藏恢复材料'));
            await enter(recoveryField, recovery);
            await tap(find.text('确认恢复材料'));
            await idle();
            await enter(passwordField, password);
            await tap(find.text('写入合成账务'));
            await idle();
            Future<Map<String, dynamic>> inspect() async {
              final result = await service!.core({
                'kind': 'inspect',
                'unlock': 'recovery',
              }, Uint8List.fromList(recovery!));
              try {
                result.requireSuccess();
                return result.report;
              } finally {
                result.clear();
              }
            }

            phase('baseline');
            final baseline = await inspect();
            final audit = baseline['audit'] as Map;
            expect(audit['sequence'], 8);
            expect(audit['revisions'], 8);
            expect((audit['operations'] as List).length, 7);
            expect((audit['snapshot']['deletedEventIds'] as List).length, 1);
            expect(
              (audit['snapshot']['unresolvedCandidateRevisionIds'] as List)
                  .length,
              2,
            );
            expect(audit['snapshot']['netAssetsMinor'], '18000');
            await enter(
              passwordField,
              Uint8List.fromList(utf8.encode('incorrect synthetic input')),
            );
            await tap(find.byKey(const ValueKey('m0c-unlock-password')));
            await idle();
            await reveal(find.byKey(const ValueKey('m0c-error')));
            expect(find.textContaining('解锁材料不正确'), findsOneWidget);
            phase('cancel-unlock');
            await enter(passwordField, password);
            await tap(find.byKey(const ValueKey('m0c-unlock-password')));
            expect(
              tester
                  .widget<IconButton>(
                    find.byKey(const ValueKey('m0c-evidence')),
                  )
                  .onPressed,
              isNull,
              reason: 'cancel while the actual unlock is still pending',
            );
            await tester.tap(find.byTooltip('锁定'));
            await idle();
            expect(find.byKey(const ValueKey('m0c-success')), findsNothing);
            await assertInputsCleared();
            await enter(recoveryField, recovery);
            await tap(find.byKey(const ValueKey('m0c-unlock-recovery')));
            await idle();
            expect(
              (await inspect())['contentDigest'],
              baseline['contentDigest'],
            );
            phase('cancel-cleanup');
            // 明确取消清理对话框，不改变 selection / 原账务。
            await tap(find.byKey(const ValueKey('m0c-cleanup')));
            // 确认期间页面仍忙碌，不能等待底层进度动画停止。
            await waitUntil(
              () => find.text('确认本次操作').hitTestable().evaluate().isNotEmpty,
            );
            await tester.tap(find.text('取消'));
            await idle();
            expect((await BootstrapSelection.load(root))!.run, service.run);
            await enter(passwordField, password);
            phase('prepare-review');
            await tap(find.byKey(const ValueKey('m0c-review-expand')));
            await settle();
            await tap(
              find.byKey(const ValueKey('m0c-review-prepare-password')),
            );
            await idle();
            await assertInputsCleared();
            await tap(find.byKey(const ValueKey('m0c-review-observe')));
            await idle();
            // 现有模拟器不声称文件锁屏拒绝；真实 OS 后台切换由宿主单独执行。
            phase('background');
            final lifecycle = LifecycleEvidence();
            WidgetsBinding.instance.addObserver(lifecycle);
            try {
              await enter(passwordField, password);
              await enter(recoveryField, recovery);
              await markerPending.create(exclusive: true);
              await markerPending.writeAsString(
                jsonEncode({
                  'session': session,
                  'run': service.run,
                  'bridge': bridge.name,
                  'pid': pid,
                }),
                flush: true,
              );
              await markerPending.rename(marker.path);
              await waitUntil(
                () =>
                    lifecycle.states.contains(AppLifecycleState.paused) &&
                    lifecycle.states.last == AppLifecycleState.resumed,
              );
              await marker.delete();
            } finally {
              WidgetsBinding.instance.removeObserver(lifecycle);
            }
            await assertInputsCleared();
            await enter(recoveryField, recovery);
            await tap(find.byKey(const ValueKey('m0c-review-verify-recovery')));
            await idle();
            await reveal(find.byKey(const ValueKey('m0c-review-state')));
            expect(find.textContaining('同一账库复核一致'), findsOneWidget);
            final reopened = await inspect();
            expect(reopened['contentDigest'], baseline['contentDigest']);
            expect(reopened['vault'], baseline['vault']);
            expect(reopened['device'], baseline['device']);
            expect(reopened['audit'], audit);
            expect(reopened['pid'], pid);
            phase('review-screenshot');
            await screenshot(bridge, 'review');
            tester.platformDispatcher.textScaleFactorTestValue = 2;
            try {
              await settle();
              await screenshot(bridge, 'large-text');
              expect(tester.takeException(), isNull);
            } finally {
              tester.platformDispatcher.clearTextScaleFactorTestValue();
              await settle();
            }
            phase('diagnostics');
            await tester.tap(find.byKey(const ValueKey('m0c-evidence')));
            await settle();
            final diagnostic =
                jsonDecode(
                      tester
                          .widget<SelectableText>(
                            find.byKey(const ValueKey('m0c-evidence-json')),
                          )
                          .data!,
                    )
                    as Map;
            expect(diagnostic['hardwareVerified'], isFalse);
            final observations = diagnostic['observations'] as List;
            expect(
              observations.any(
                (e) =>
                    e['action'] == 'unlockPassword' &&
                    e['waitStopped'] == true &&
                    e['outcome'] == 'succeeded',
              ),
              isTrue,
            );
            final verified = observations.lastWhere(
              (e) => e['action'] == 'lockVerify',
            )['summary']['lockReview'];
            expect(verified['reviewState'], 'matched');
            expect(verified['baselineDigest'], baseline['contentDigest']);
            expect(verified['afterDigest'], baseline['contentDigest']);
            expect(
              verified['fileOutcome'],
              Platform.isIOS ? 'unsupportedSimulator' : 'unsupportedPlatform',
            );
            await tester.tap(
              find.byKey(const ValueKey('m0c-publish-evidence')),
            );
            await waitUntil(
              () => find.textContaining('分片已发出').evaluate().isNotEmpty,
            );
            await tester.tap(find.text('关闭'));
            await settle();
            phase('cleanup-retry');
            // 可恢复的结构故障：原核心拒绝目录项，不冒充真实磁盘 IO 故障。
            obstacle = Directory(
              '$root/m0c-${service.run}/ui-cleanup-obstacle',
            );
            expect(await obstacle.exists(), isFalse);
            await obstacle.create();
            await tap(find.byKey(const ValueKey('m0c-cleanup')));
            // 确认期间页面仍忙碌，不能等待底层进度动画停止。
            await waitUntil(
              () => find.text('确认本次操作').hitTestable().evaluate().isNotEmpty,
            );
            await tester.tap(find.text('确认本次操作'));
            await idle();
            expect(
              (await BootstrapSelection.load(root))!.cleanupCandidates,
              isNotNull,
            );
            await screenshot(bridge, 'cleanup-retry');
            await tester.pumpWidget(const SizedBox.shrink());
            await obstacle.delete();
            obstacle = null;
            await tester.pumpWidget(const BootstrapApp());
            await reveal(find.byKey(const ValueKey('m0c-cleanup')));
            await waitUntil(
              () =>
                  tester
                      .widget<OutlinedButton>(
                        find.byKey(const ValueKey('m0c-cleanup')),
                      )
                      .onPressed !=
                  null,
            );
            expect(find.text('重试本次清理'), findsOneWidget);
            await tap(find.byKey(const ValueKey('m0c-cleanup')));
            await idle();
            expect(await BootstrapSelection.load(root), isNull);
            expect(
              await Directory('$root/m0c-${service.run}').exists(),
              isFalse,
            );
            cleaned = true;
            (reports['ownedRuns'] as List<String>).remove(service.run);
            reports[bridge.name] = {
              'fixture': 'A11',
              'uiCreatedAndConfirmed': true,
              'wrongMaterialPreservedFacts': true,
              'sameVaultReviewMatched': true,
              'fileOutcome': verified['fileOutcome'],
              'actualLifecycle': lifecycle.states.map((e) => e.name).toList(),
              'samePid': pid,
              'inputsClearedAfterBackground': true,
              'twoTimesTextScale': 'testOverrideOnSimulator',
              'cancelLateResultSuppressed': true,
              'cleanupCancelled': true,
              'cleanupStructuralFailureRetriedAfterPageReopen': true,
              'selectionRemoved': true,
              'diagnosticReportId': diagnostic['reportId'],
              'systemProtection':
                  'notRunAuthenticationRequiresSeparateDeviceConditions',
            };
            binding.reportData!.addAll(reports);
          } finally {
            password.fillRange(0, password.length, 0);
            recovery?.fillRange(0, recovery.length, 0);
            if (obstacle != null && await obstacle.exists()) {
              await obstacle.delete();
            }
            if (await marker.exists()) await marker.delete();
            if (await markerPending.exists()) await markerPending.delete();
            if (!cleaned && service != null) {
              await tester.pumpWidget(const SizedBox.shrink());
              final selection = await BootstrapSelection.load(root);
              if (selection != null) {
                selection.requireOwner(service);
                (await service.cleanupSelection()).clear();
                (reports['ownedRuns'] as List<String>).remove(service.run);
              }
            }
            binding.reportData!.addAll(reports);
          }
        }
        reports['completed'] = true;
        reports['cleanupComplete'] = (reports['ownedRuns'] as List).isEmpty;
      } finally {
        await tester.pumpWidget(const SizedBox.shrink());
        binding.reportData!.addAll(reports);
      }
    },
    timeout: const Timeout(Duration(minutes: 10)),
  );
}
