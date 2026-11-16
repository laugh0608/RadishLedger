import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/internal_ledger_app.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/internal_ledger.dart';

const _password = 'synthetic-internal-ledger-password';

class UnavailableSystem extends SystemProtection {
  UnavailableSystem(this.directory);
  final Directory directory;

  @override
  Future<String> internalLedgerStorage() async => directory.path;

  @override
  Future<Map<String, dynamic>> capabilities() async => {'supported': false};

  @override
  Future<Uint8List> read(String run, String generation) async =>
      throw StateError('system authentication is unavailable');
}

Uint8List _bytes(String value) => Uint8List.fromList(utf8.encode(value));

Matcher _failed(String code) =>
    isA<BootstrapFailure>().having((error) => error.code, 'code', code);

Future<Map<String, dynamic>> _report(Future<BootstrapResult> call) async {
  final result = await call;
  try {
    result.requireSuccess();
    return result.report;
  } finally {
    result.clear();
  }
}

Future<void> _until(WidgetTester tester, bool Function() condition) async {
  for (var count = 0; count < 250; count++) {
    await tester.pump(const Duration(milliseconds: 20));
    if (condition()) return;
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 20)),
    );
  }
  fail('internal ledger did not reach the expected state');
}

void main() {
  test('thin FFI binds mutations and reopens committed A11 facts', () async {
    final root = await Directory.systemTemp.createTemp('radish_internal_');
    addTearDown(() => root.delete(recursive: true));
    final platform = UnavailableSystem(root);
    final first = await InternalLedger.open(platform);
    expect((await first.locate()).state, InternalLedgerState.empty);
    expect(await root.list().isEmpty, isTrue);

    final password = _bytes(_password);
    final created = await first.create(password);
    addTearDown(created.clear);
    expect(password.every((byte) => byte == 0), isTrue);
    expect(created.report['ready'], false);
    final identity = InternalLedger.identity(created.report);

    final wrongIdentity = ('11111111111111111111111111111111', identity.$2);
    await expectLater(
      first.confirm(wrongIdentity, Uint8List.fromList(created.secret)),
      throwsA(_failed('selection_changed')),
    );
    final onDisk = await first.locate();
    final unconfirmed = await _report(
      first.unlock(onDisk, LedgerUnlock.password, _bytes(_password)),
    );
    expect(unconfirmed['ready'], false);
    final ready = await _report(
      first.confirm(identity, Uint8List.fromList(created.secret)),
    );
    expect(ready['ready'], true);

    final seeded = await _report(
      first.service.core({
        'kind': 'seed',
        'unlock': 'recovery',
      }, Uint8List.fromList(created.secret)),
    );
    final audit = seeded['audit'] as Map;
    expect(audit['sequence'], 8);
    expect(audit['revisions'], 8);
    expect(audit['operations'], hasLength(7));
    final snapshot = audit['snapshot'] as Map;
    expect(snapshot['netAssetsMinor'], '18000');
    expect(snapshot['deletedEventIds'], hasLength(1));
    expect(snapshot['unresolvedCandidateRevisionIds'], hasLength(2));

    final reopened = await InternalLedger.open(platform);
    final location = await reopened.locate();
    expect(location.identity, identity);
    final wrongPassword = _bytes('wrong synthetic password');
    await expectLater(
      reopened.unlock(location, LedgerUnlock.password, wrongPassword),
      throwsA(_failed('authentication_failed')),
    );
    expect(wrongPassword.every((byte) => byte == 0), isTrue);
    final opened = await _report(
      reopened.unlock(location, LedgerUnlock.password, _bytes(_password)),
    );
    expect(InternalLedger.identity(opened), identity);
    expect(opened['contentDigest'], seeded['contentDigest']);
    expect(opened['audit'], audit);
  });

  testWidgets(
    'create, confirm, close and reopen on a narrow large-text screen',
    (tester) async {
      final root = Directory.systemTemp.createTempSync(
        'radish_internal_widget_',
      );
      addTearDown(() => root.deleteSync(recursive: true));
      final platform = UnavailableSystem(root);
      tester.view.physicalSize = const Size(320, 568);
      tester.view.devicePixelRatio = 1;
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);

      Future<void> tapLabel(String label) async {
        final item = find.widgetWithText(FilledButton, label);
        await tester.ensureVisible(item);
        await tester.tap(item);
        await tester.pump();
      }

      await tester.pumpWidget(InternalLedgerApp(platform: platform));
      await _until(tester, () => find.text('创建本地账簿').evaluate().isNotEmpty);
      await tester.enterText(find.byType(TextField), _password);
      await tapLabel('创建账簿');
      await _until(tester, () => find.text('确认恢复材料').evaluate().isNotEmpty);
      final recovery = tester
          .widget<SelectableText>(find.byType(SelectableText))
          .data!;
      final field = find.byType(TextField);
      await tester.ensureVisible(field);
      await tester.enterText(field, recovery);
      await tapLabel('确认已保存的恢复材料');
      await _until(tester, () => find.text('账簿已打开').evaluate().isNotEmpty);
      expect(find.byType(SelectableText), findsNothing);
      await tapLabel('关闭账簿');
      expect(find.text('解锁本地账簿'), findsOneWidget);
      expect(find.textContaining('设备认证不可用'), findsOneWidget);

      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpWidget(InternalLedgerApp(platform: platform));
      await _until(tester, () => find.text('解锁本地账簿').evaluate().isNotEmpty);
      await tester.enterText(field, 'wrong synthetic password');
      await tapLabel('打开账簿');
      await _until(
        tester,
        () => find.textContaining('不正确').evaluate().isNotEmpty,
      );
      expect(find.text('账簿已打开'), findsNothing);

      await tester.enterText(field, _password);
      await tapLabel('打开账簿');
      final controller = tester.widget<TextField>(field).controller!;
      for (final state in [
        AppLifecycleState.inactive,
        AppLifecycleState.hidden,
        AppLifecycleState.paused,
      ]) {
        tester.binding.handleAppLifecycleStateChanged(state);
      }
      expect(controller.text.isEmpty, isTrue);
      for (final state in [
        AppLifecycleState.hidden,
        AppLifecycleState.inactive,
        AppLifecycleState.resumed,
      ]) {
        tester.binding.handleAppLifecycleStateChanged(state);
      }
      await tester.pump();
      await _until(
        tester,
        () => find.byType(LinearProgressIndicator).evaluate().isEmpty,
      );
      expect(find.text('账簿已打开'), findsNothing);
      expect(controller.text.isEmpty, isTrue);

      final switchToRecovery = find.text('改用恢复材料');
      await tester.ensureVisible(switchToRecovery);
      await tester.tap(switchToRecovery);
      await tester.pump();
      await tester.enterText(field, recovery);
      await tapLabel('打开账簿');
      await _until(tester, () => find.text('账簿已打开').evaluate().isNotEmpty);
      expect(tester.takeException(), isNull);
    },
  );
}
