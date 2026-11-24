import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/internal_ledger.dart';
import 'package:radishledger_probe/src/restore_archive.dart';
import 'package:radishledger_probe/src/restore_page.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  testWidgets('系统选择取消、独立向量恢复、错口令及后台收回暂存', (tester) async {
    final root = Directory.systemTemp.createTempSync('rl_restore_');
    addTearDown(() => root.deleteSync(recursive: true));
    final vector = jsonDecode(
      File(
        '../../docs/validation/archive-container-review-inputs.json',
      ).readAsStringSync(),
    );
    final hex = vector['cryptoExpected']['archiveHex'] as String;
    final cipher = <int>[
      for (int i = 0; i < hex.length; i += 2)
        int.parse(hex.substring(i, i + 2), radix: 16),
    ];
    final stage = 'archive-${'1' * 32}';
    final dir = Directory('${root.path}/$stage');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    var choose = false;
    Completer<void>? copying;
    messenger.setMockMethodCallHandler(archivePicker, (_) async => choose);
    messenger.setMockMethodCallHandler(archiveInput, (call) async {
      expect(call.arguments['limit'], greaterThan(cipher.length));
      expect(call.arguments['magic'], cipher.take(8).toList());
      dir.createSync();
      File('${dir.path}/source.rla').writeAsBytesSync(cipher);
      if (copying != null) await copying.future;
      return {'directory': root.path, 'stage': stage};
    });
    addTearDown(() {
      messenger.setMockMethodCallHandler(archivePicker, null);
      messenger.setMockMethodCallHandler(archiveInput, null);
    });
    final ledger = InternalLedger(root.path, const SystemProtection());
    await tester.pumpWidget(
      MaterialApp(
        home: RestorePage(ledger: ledger, protectionAvailable: false),
      ),
    );
    Future<void> tap(String label) async {
      final element = find.text(label);
      await tester.ensureVisible(element);
      await tester.tap(element);
      await tester.pump();
    }

    Future<void> until(bool Function() done) async {
      for (var count = 0; count < 300; count++) {
        await tester.pump(const Duration(milliseconds: 20));
        if (done()) return;
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 20)),
        );
      }
      fail('恢复预期状态未出现');
    }

    await tap('选择本机归档');
    await tester.pumpAndSettle();
    expect(dir.existsSync(), false);
    choose = true;
    await tap('选择本机归档');
    await until(() => find.byType(TextField).evaluate().isNotEmpty);
    await tester.enterText(find.byType(TextField), 'wrong');
    await tap('恢复到空暂存并核对');
    await until(() => find.textContaining('归档口令不正确').evaluate().isNotEmpty);
    expect(dir.listSync().length, 1);
    await tester.enterText(
      find.byType(TextField),
      vector['baseInput']['passwordUtf8'] as String,
    );
    await tap('恢复到空暂存并核对');
    await until(() => find.text('恢复暂存已核对').evaluate().isNotEmpty);
    expect(find.text('净资产 ¥0.00'), findsOneWidget);
    expect(dir.listSync().length, 2);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await until(
      () => !dir.existsSync() && find.text('选择本机归档').evaluate().isNotEmpty,
    );
    expect(find.text('恢复暂存已核对'), findsNothing);
    copying = Completer<void>();
    await tap('选择本机归档');
    await until(dir.existsSync);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    copying.complete();
    await until(
      () => !dir.existsSync() && find.text('已清理本次恢复暂存。').evaluate().isNotEmpty,
    );
    expect(find.byType(TextField), findsNothing);
    expect(
      await tester.runAsync(() async => (await ledger.locate()).state),
      InternalLedgerState.empty,
    );
  });
}
