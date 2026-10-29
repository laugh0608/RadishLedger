import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'bootstrap_lock_review_test.dart'
    show FileObservationProtection, InspectService, syntheticInspect;
import 'bootstrap_widget_test.dart' show mount;

Future<void> reveal(
  WidgetTester tester,
  Finder finder, {
  double step = 240,
}) async {
  for (var i = 0; i < 60; i++) {
    if (finder.evaluate().isNotEmpty) {
      await tester.ensureVisible(finder);
      await tester.pump();
      if (finder.hitTestable().evaluate().isNotEmpty) return;
    }
    await tester.drag(find.byType(ListView), Offset(0, -step));
    await tester.pump();
  }
  fail('Control not reachable by scrolling: $finder');
}

Future<void> tapReview(WidgetTester tester, String key) async {
  final finder = find.byKey(ValueKey(key));
  await reveal(tester, finder);
  await tester.tap(finder);
  await tester.pump();
}

Future<void> expand(WidgetTester tester) async {
  await tapReview(tester, 'm0c-review-expand');
  await tester.pumpAndSettle();
}

Future<void> background(WidgetTester tester) async {
  for (final state in [
    AppLifecycleState.inactive,
    AppLifecycleState.hidden,
    AppLifecycleState.paused,
  ]) {
    tester.binding.handleAppLifecycleStateChanged(state);
  }
  await tester.pump();
  expect(find.text('已锁定'), findsOneWidget);
}

Future<void> resume(WidgetTester tester) async {
  for (final state in [
    AppLifecycleState.hidden,
    AppLifecycleState.inactive,
    AppLifecycleState.resumed,
  ]) {
    tester.binding.handleAppLifecycleStateChanged(state);
  }
  await tester.pump();
}

class LateInspectService extends InspectService {
  LateInspectService(super.platform);
  Completer<BootstrapResult>? next;
  @override
  Future<BootstrapResult> core(
    Map<String, Object?> action,
    Uint8List input, {
    String? pauseAt,
    Map<String, String>? fileFault,
  }) {
    if (next != null) {
      input.fillRange(0, input.length, 0);
      return next!.future;
    }
    return super.core(action, input, pauseAt: pauseAt, fileFault: fileFault);
  }
}

void main() {
  testWidgets(
    'background observation clears input, retains original run report, and requires fresh unlock',
    (tester) async {
      final platform = FileObservationProtection()
        ..pendingObservation = Completer<Map<String, dynamic>>();
      final service = InspectService(platform);
      await mount(tester, service);
      final password = find.byKey(const ValueKey('m0c-password'));
      final recovery = find.byKey(const ValueKey('m0c-recovery'));
      await tester.enterText(password, 'synthetic-before-lock');
      await tester.enterText(recovery, 'synthetic-unused-material');
      await expand(tester);
      await tapReview(tester, 'm0c-review-prepare-password');
      await tester.pumpAndSettle();
      await tapReview(tester, 'm0c-review-observe');
      await background(tester);
      expect(platform.pendingObservation!.isCompleted, isFalse);
      final native = platform.fileReport(service.run, platform.observedId!);
      platform.pendingObservation!.complete(native);
      await tester.pump();
      await resume(tester);
      await tester.pumpAndSettle();
      // The hidden page was rebuilt; no retained editable text survives.
      await reveal(tester, password, step: -240);
      expect(tester.widget<TextField>(password).controller!.text, isEmpty);
      expect(tester.widget<TextField>(recovery).controller!.text, isEmpty);
      await tester.enterText(recovery, 'synthetic-fresh-recovery');
      await tapReview(tester, 'm0c-review-verify-recovery');
      await tester.pumpAndSettle();
      expect(find.textContaining('同一账库复核一致'), findsOneWidget);
      expect(find.textContaining('超时，未执行锁定阶段 IO'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('m0c-evidence')));
      await tester.pumpAndSettle();
      final text = tester
          .widget<SelectableText>(
            find.byKey(const ValueKey('m0c-evidence-json')),
          )
          .data!;
      final records = jsonDecode(text)['observations'] as List;
      final observation = records.singleWhere(
        (e) => e['action'] == 'lockObserve',
      );
      expect(observation['run'], service.run);
      expect(observation['bridge'], 'thin');
      expect(observation['waitStopped'], isTrue);
      expect(
        observation['summary']['lockReview']['walCoverage'],
        'absentAtStart',
      );
      expect(text, isNot(contains('synthetic-fresh-recovery')));
      expect(text, isNot(contains('synthetic-before-lock')));
      expect(text, isNot(contains('never-retain-native-extras')));
      await tester.tap(find.text('关闭'));
      await tester.pumpAndSettle();
      await tester.pumpWidget(const SizedBox());
      await mount(tester, service);
      await expand(tester);
      expect(find.textContaining('当前没有复核基线'), findsOneWidget);
      final verify = find.byKey(const ValueKey('m0c-review-verify-recovery'));
      await reveal(tester, verify);
      expect(tester.widget<OutlinedButton>(verify).onPressed, isNull);
    },
  );

  testWidgets(
    'explicit cancellation preserves baseline; failed unlock can retry; mismatch never seeds',
    (tester) async {
      final platform = FileObservationProtection()
        ..pendingObservation = Completer<Map<String, dynamic>>();
      final service = InspectService(platform);
      await mount(tester, service);
      await expand(tester);
      await tapReview(tester, 'm0c-review-prepare-recovery');
      await tester.pumpAndSettle();
      await tapReview(tester, 'm0c-review-observe');
      await tester.tap(find.byTooltip('锁定'));
      await tester.pumpAndSettle();
      expect(find.textContaining('文件观测：已取消'), findsOneWidget);
      service.fail = const BootstrapFailure('authentication_failed');
      await tapReview(tester, 'm0c-review-verify-password');
      await tester.pumpAndSettle();
      expect(find.textContaining('重新解锁未完成'), findsOneWidget);
      service.fail = null;
      service.data = {
        ...syntheticInspect(),
        'contentDigest': List.filled(64, 'b').join(),
      };
      await tapReview(tester, 'm0c-review-verify-recovery');
      await tester.pumpAndSettle();
      expect(find.textContaining('复核不一致，保留原库和基线'), findsOneWidget);
      expect(service.actions, everyElement('inspect'));
      final prepare = find.byKey(const ValueKey('m0c-review-prepare-recovery'));
      expect(tester.widget<OutlinedButton>(prepare).onPressed, isNull);
    },
  );

  testWidgets(
    'late verification cannot become matched after locking; a replacement page has no baseline',
    (tester) async {
      final platform = FileObservationProtection();
      final service = LateInspectService(platform);
      await mount(tester, service);
      await expand(tester);
      await tapReview(tester, 'm0c-review-prepare-password');
      await tester.pumpAndSettle();
      await tapReview(tester, 'm0c-review-observe');
      await tester.pumpAndSettle();
      service.next = Completer<BootstrapResult>();
      await tapReview(tester, 'm0c-review-verify-password');
      await tester.tap(find.byTooltip('锁定'));
      final output = Uint8List.fromList(List.filled(32, 17));
      service.next!.complete(
        BootstrapResult({'ok': true, 'report': syntheticInspect()}, output),
      );
      await tester.pumpAndSettle();
      expect(find.textContaining('同一账库复核一致'), findsNothing);
      expect(find.textContaining('复核等待已停止'), findsOneWidget);
      expect(output, everyElement(0));
      await tester.pumpWidget(const SizedBox());
      await mount(tester, InspectService(FileObservationProtection()));
      await expand(tester);
      expect(find.textContaining('当前没有复核基线'), findsOneWidget);
    },
  );

  testWidgets(
    'large text review controls remain scrollable without layout overflow',
    (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      tester.view.physicalSize = const Size(390, 844);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final service = InspectService(FileObservationProtection());
      await mount(tester, service);
      // 窄屏初始状态文本尚未懒加载；等待实际输入启用，不能用文本不可见代替初始化完成。
      await tester.runAsync(() async {
        final deadline = DateTime.now().add(const Duration(seconds: 5));
        while (DateTime.now().isBefore(deadline)) {
          await Future<void>.delayed(const Duration(milliseconds: 5));
          await tester.pump();
          if (tester
                  .widget<TextField>(find.byKey(const ValueKey('m0c-password')))
                  .enabled ==
              true) {
            return;
          }
        }
        fail('Review page initialization did not complete');
      });
      await expand(tester);
      await tapReview(tester, 'm0c-review-prepare-password');
      await tester.pumpAndSettle();
      await tapReview(tester, 'm0c-review-observe');
      await tester.pumpAndSettle();
      await tapReview(tester, 'm0c-review-verify-password');
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );
}
