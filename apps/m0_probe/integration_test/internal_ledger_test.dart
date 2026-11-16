import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:radishledger_probe/internal_ledger_app.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/internal_ledger.dart';

// 仅替换测试账簿的位置，平台能力由真实 Android 通道返回。
class _OwnedDirectory extends SystemProtection {
  const _OwnedDirectory(this.directory);

  final Directory directory;

  @override
  Future<String> internalLedgerStorage() async => directory.path;
}

Future<void> _wait(WidgetTester tester, bool Function() ready) async {
  final deadline = DateTime.now().add(const Duration(seconds: 25));
  while (DateTime.now().isBefore(deadline)) {
    await tester.pump(const Duration(milliseconds: 50));
    if (ready()) return;
  }
  throw StateError('页面未在限定时间进入预期状态');
}

Future<void> _tap(WidgetTester tester, String label) async {
  final control = find.text(label);
  await tester.ensureVisible(control);
  await _wait(tester, () => control.hitTestable().evaluate().length == 1);
  await tester.tap(control, warnIfMissed: false);
  await tester.pump();
}

Future<void> _enter(WidgetTester tester, Uint8List input) async {
  final field = find.byType(TextField);
  await tester.ensureVisible(field);
  final widget = tester.widget<TextField>(field);
  if (widget.enabled != true || widget.controller == null) {
    throw StateError('账簿输入当前不可用');
  }
  // 只在测试进程内设置输入。真实 IME 的异步编辑状态属于独立验收项。
  widget.controller!.text = utf8.decode(input);
  await tester.pump();
}

Future<Map<String, dynamic>> _report(Future<BootstrapResult> call) async {
  final result = await call;
  try {
    result.requireSuccess();
    return result.report;
  } finally {
    result.clear();
  }
}

void _verifyA11(Map<String, dynamic> report) {
  final audit = report['audit'] as Map;
  final snapshot = audit['snapshot'] as Map;
  final balances = (snapshot['balancesMinor'] as Map).values.toList()..sort();
  expect(audit['sequence'], 8);
  expect(audit['revisions'], 8);
  expect(audit['operations'], hasLength(7));
  expect(snapshot['activeEventIds'], hasLength(3));
  expect(snapshot['selectedRevisionIds'], hasLength(4));
  expect(snapshot['deletedEventIds'], hasLength(1));
  expect(snapshot['unresolvedCandidateRevisionIds'], hasLength(2));
  expect(snapshot['netAssetsMinor'], '18000');
  expect(balances, ['10000', '8000']);
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('Android internal ledger confirms recovery and reopens A11', (
    tester,
  ) async {
    expect(Platform.isAndroid, isTrue, reason: '需要实际 Android 运行环境');
    const system = SystemProtection();
    final root = Directory(await system.internalLedgerStorage());
    final capability = await system.capabilities();
    final supported = capability['supported'] == true;
    final directory = await root.createTemp('ui-validation-');
    final platform = _OwnedDirectory(directory);
    final ledger = await InternalLedger.open(platform);
    final random = Random.secure();
    final password = Uint8List.fromList(
      List.generate(48, (_) => 97 + random.nextInt(26)),
    );
    Uint8List? recovery;
    var phase = '创建';
    var verified = false;
    try {
      expect((await ledger.locate()).state, InternalLedgerState.empty);
      try {
        await tester.pumpWidget(InternalLedgerApp(platform: platform));
        await _wait(tester, () => find.text('创建本地账簿').evaluate().isNotEmpty);
        await _enter(tester, password);
        await _tap(tester, '创建账簿');
        phase = '确认恢复材料';
        await _wait(tester, () => find.text('确认恢复材料').evaluate().isNotEmpty);
        final displayed = tester
            .widget<SelectableText>(find.byType(SelectableText))
            .data;
        if (displayed == null || displayed.length != 80) {
          throw StateError('恢复材料长度异常');
        }
        recovery = Uint8List.fromList(utf8.encode(displayed));
        await _enter(tester, recovery);
        await _tap(tester, '确认已保存的恢复材料');
        await _wait(tester, () => find.text('账簿已打开').evaluate().isNotEmpty);
      } catch (_) {
        // Finder 或输入失败时先卸载持有恢复材料的界面，再报告固定阶段。
        await tester.pumpWidget(const SizedBox.shrink());
        fail('Android 内部账簿未完成阶段 $phase');
      }

      final original = await ledger.locate();
      final before = await _report(
        ledger.service.core({
          'kind': 'seed',
          'unlock': 'recovery',
        }, Uint8List.fromList(recovery)),
      );
      _verifyA11(before);
      expect(before['ready'], isTrue);
      expect(before['fixtureComplete'], isTrue);
      expect(before['pid'], pid);
      expect(InternalLedger.identity(before), original.identity);
      await _tap(tester, '关闭账簿');
      expect(find.text('解锁本地账簿'), findsOneWidget);
      expect(
        find.textContaining('设备认证不可用'),
        supported ? findsNothing : findsOneWidget,
      );

      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpWidget(InternalLedgerApp(platform: platform));
      await _wait(tester, () => find.text('解锁本地账簿').evaluate().isNotEmpty);
      await _enter(
        tester,
        Uint8List.fromList(utf8.encode('incorrect fixture')),
      );
      await _tap(tester, '打开账簿');
      await _wait(
        tester,
        () => find.textContaining('不正确').evaluate().isNotEmpty,
      );
      expect(find.text('账簿已打开'), findsNothing);

      phase = '切换至独立材料输入';
      try {
        await tester.pumpAndSettle();
        await _tap(tester, '改用恢复材料');
        await _wait(
          tester,
          () =>
              tester
                  .widget<TextField>(find.byType(TextField))
                  .decoration
                  ?.labelText ==
              '恢复材料',
        );
        phase = '输入独立材料';
        await _enter(tester, recovery);
        phase = '提交独立材料';
        await _tap(tester, '打开账簿');
        phase = '等待独立材料重开';
        await _wait(tester, () => find.text('账簿已打开').evaluate().isNotEmpty);
      } catch (_) {
        await tester.pumpWidget(const SizedBox.shrink());
        fail('Android 内部账簿未完成阶段 $phase');
      }
      final located = await ledger.locate();
      final after = await _report(
        ledger.unlock(
          located,
          LedgerUnlock.password,
          Uint8List.fromList(password),
        ),
      );
      expect(InternalLedger.identity(after), original.identity);
      expect(after['contentDigest'], before['contentDigest']);
      expect(after['audit'], before['audit']);
      expect(after['pid'], pid);
      verified = true;
    } finally {
      recovery?.fillRange(0, recovery.length, 0);
      password.fillRange(0, password.length, 0);
      await tester.pumpWidget(const SizedBox.shrink());
      // 未取得确认时，保全独占目录供复核，避免与尚未结束的核心调用竞争。
      if (verified) await directory.delete(recursive: true);
    }
    expect(await directory.exists(), isFalse);
    debugPrint(
      'RADISH_INTERNAL_LEDGER_EVIDENCE ${jsonEncode({'pid': pid, 'deviceSecure': capability['deviceSecure'], 'deviceAuthenticationSupported': supported, 'platformCode': capability['platformCode'], 'sameIdentityAndAudit': true, 'fixture': 'A11', 'cleanupComplete': true})}',
    );
  });
}
