import 'dart:async';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/bootstrap_app.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/probe.dart';

class TestProtection extends SystemProtection {
  int cancelled = 0;
  @override
  Future<Map<String, dynamic>> capabilities() async => {'supported': false};
  @override
  Future<void> cancel() async {
    cancelled++;
  }
}

class ControlledService extends BootstrapService {
  ControlledService(TestProtection platform)
    : super(
        '/synthetic',
        '00000000000000000000000000000000',
        BridgeCandidate.thin,
        platform,
      );
  Completer<BootstrapResult> pending = Completer<BootstrapResult>();
  int calls = 0;
  @override
  Future<BootstrapResult> core(
    Map<String, Object?> action,
    Uint8List input, {
    String? pauseAt,
  }) {
    calls++;
    input.fillRange(0, input.length, 0);
    return pending.future;
  }
}

void main() {
  testWidgets(
    'cancel suppresses late unlock and keeps input disabled until completion',
    (tester) async {
      final platform = TestProtection();
      final service = ControlledService(platform);
      await tester.pumpWidget(BootstrapApp(service: service));
      await tester.pumpAndSettle();
      final unlock = find.byKey(const ValueKey('m0c-unlock-password'));
      await tester.ensureVisible(unlock);
      await tester.tap(unlock);
      await tester.pump();
      final cancel = find.byKey(const ValueKey('m0c-cancel'));
      await tester.ensureVisible(cancel);
      await tester.tap(cancel);
      await tester.pump();
      expect(service.calls, 1);
      expect(platform.cancelled, 1);
      expect(tester.widget<FilledButton>(unlock).onPressed, isNull);
      final output = Uint8List.fromList(List.filled(80, 65));
      service.pending.complete(
        BootstrapResult({
          'ok': true,
          'report': {
            'fixtureComplete': true,
            'audit': {'operations': []},
          },
        }, output),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('m0c-success')), findsNothing);
      expect(find.byKey(const ValueKey('m0c-issued')), findsNothing);
      expect(output.every((b) => b == 0), isTrue);
      expect(tester.widget<FilledButton>(unlock).onPressed, isNotNull);
    },
  );

  testWidgets('failure remains actionable and background clears inputs', (
    tester,
  ) async {
    final platform = TestProtection();
    final service = ControlledService(platform);
    await tester.pumpWidget(BootstrapApp(service: service));
    await tester.pumpAndSettle();
    final password = find.byKey(const ValueKey('m0c-password'));
    await tester.enterText(password, 'synthetic nonsecret test input');
    final unlock = find.byKey(const ValueKey('m0c-unlock-password'));
    await tester.ensureVisible(unlock);
    await tester.tap(unlock);
    service.pending.completeError(
      const BootstrapFailure('authentication_failed'),
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('解锁材料不正确'), findsOneWidget);
    expect(find.byKey(const ValueKey('m0c-success')), findsNothing);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    await tester.pump();
    expect(find.text('已锁定'), findsOneWidget);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(password).controller!.text, isEmpty);
    expect(find.byKey(const ValueKey('m0c-error')), findsNothing);
    expect(platform.cancelled, greaterThan(0));
  });
}
