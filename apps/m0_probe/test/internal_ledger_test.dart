import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/internal_ledger_app.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/internal_ledger.dart';
import 'package:radishledger_probe/src/monthly_summary.dart';

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

  testWidgets(
    'reopens both entries and requires a fresh revision after a concurrent edit',
    (tester) async {
      final root = Directory.systemTemp.createTempSync(
        'radish_local_book_widget_',
      );
      addTearDown(() => root.deleteSync(recursive: true));
      final platform = UnavailableSystem(root);
      await tester.runAsync(() async {
        final ledger = await InternalLedger.open(platform);
        final created = await ledger.create(_bytes(_password));
        try {
          await _report(
            ledger.confirm(
              InternalLedger.identity(created.report),
              Uint8List.fromList(created.secret),
            ),
          );
        } finally {
          created.clear();
        }
      });

      Future<void> tap(Finder target) async {
        await tester.ensureVisible(target);
        await tester.tap(target);
        await tester.pump();
      }

      Finder field(String label) => find.byWidgetPredicate(
        (widget) =>
            widget is TextField && widget.decoration?.labelText == label,
      );

      Future<void> enter(String label, String input) async {
        final target = field(label);
        await tester.ensureVisible(target);
        await tester.enterText(target, input);
        await tester.pump();
      }

      Future<void> choose(String label, String value) async {
        final dropdown = find.ancestor(
          of: find.text(label),
          matching: find.byType(DropdownButtonFormField<String>),
        );
        await tap(dropdown);
        await tester.pumpAndSettle();
        await tap(find.text(value).last);
        await tester.pumpAndSettle();
      }

      Future<void> open() async {
        await _until(tester, () => find.text('解锁本地账簿').evaluate().isNotEmpty);
        await enter('本地口令', _password);
        await tap(find.widgetWithText(FilledButton, '打开账簿'));
        await _until(tester, () => find.text('账簿已打开').evaluate().isNotEmpty);
      }

      Future<void> save({String result = '已保存到本机'}) async {
        await enter('保存用本地口令', _password);
        await tap(find.widgetWithText(FilledButton, '保存到本机'));
        await _until(
          tester,
          () =>
              find.text(result).evaluate().isNotEmpty &&
              find.byType(LinearProgressIndicator).evaluate().isEmpty,
        );
        await tester.pump();
      }

      await tester.pumpWidget(InternalLedgerApp(platform: platform));
      await open();
      expect(find.text('还没有收支记录。'), findsOneWidget);
      await tap(find.widgetWithText(OutlinedButton, '新建账户'));
      await enter('账户名称', '合成银行');
      await choose('账户类型', '银行账户');
      await enter('期初金额（CNY）', '1000.00');
      await enter('生效日期（YYYY-MM-DD）', '2026-01-01');
      await save();
      expect(find.text('合成银行 · 银行账户 · ¥1000.00'), findsOneWidget);
      expect(find.text('还没有收支记录。'), findsOneWidget);

      await tap(find.widgetWithText(OutlinedButton, '新建分类'));
      await enter('分类名称', '临时分类');
      await tap(find.text('取消录入'));
      await tap(find.text('继续编辑'));
      expect(tester.widget<TextField>(field('分类名称')).controller!.text, '临时分类');
      await tap(find.text('取消录入'));
      await tap(find.text('丢弃输入'));
      expect(find.textContaining('临时分类'), findsNothing);

      await tap(find.widgetWithText(OutlinedButton, '新建分类'));
      await enter('分类名称', '合成工资');
      await choose('收支类型', '收入');
      await save();
      await tap(find.widgetWithText(OutlinedButton, '新建分类'));
      await enter('分类名称', '合成餐饮');
      await save();

      await tap(find.widgetWithText(FilledButton, '记一笔'));
      await choose('收支类型', '收入');
      await choose('分类', '合成工资');
      await enter('金额（CNY）', '2000.001');
      await enter('业务日期（YYYY-MM-DD）', '2026-01-05');
      await save(result: '人民币金额最多两位小数');
      expect(
        tester.widget<TextField>(field('金额（CNY）')).controller!.text,
        '2000.001',
      );
      expect(
        tester.widget<TextField>(field('保存用本地口令')).controller!.text,
        isEmpty,
      );
      await enter('金额（CNY）', '2000.00');
      await save();
      expect(find.text('合成银行 · 银行账户 · ¥3000.00'), findsOneWidget);

      await tap(find.widgetWithText(FilledButton, '记一笔'));
      await choose('分类', '合成餐饮');
      await enter('金额（CNY）', '30.00');
      await enter('业务日期（YYYY-MM-DD）', '2026-01-07');
      await save();
      await tap(find.widgetWithText(FilledButton, '关闭账簿'));
      expect(find.textContaining('¥2970.00'), findsNothing);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpWidget(InternalLedgerApp(platform: platform));
      await open();
      expect(find.text('最近核对的保存已提交。'), findsOneWidget);
      expect(find.text('合成银行 · 银行账户 · ¥2970.00'), findsOneWidget);
      expect(
        find.text('2026-01-05 · 收入 · ¥2000.00\n合成银行 · 合成工资'),
        findsOneWidget,
      );
      expect(
        find.text('2026-01-07 · 支出 · ¥30.00\n合成银行 · 合成餐饮'),
        findsOneWidget,
      );
      await tap(find.text('编辑这笔支出'));
      expect(
        tester.widget<TextField>(field('金额（CNY）')).controller!.text,
        '30.00',
      );
      await enter('金额（CNY）', '25.00');
      await enter('业务日期（YYYY-MM-DD）', '2026-02-01');
      await tap(find.text('取消录入'));
      await tap(find.text('继续编辑'));
      expect(
        tester.widget<TextField>(field('业务日期（YYYY-MM-DD）')).controller!.text,
        '2026-02-01',
      );

      // 另一调用者在旧表单打开时先提交，原页面须拒绝，并可重新解锁取得新修订。
      await tester.runAsync(() async {
        final ledger = await InternalLedger.open(platform);
        final location = await ledger.locate();
        final report = await _report(
          ledger.unlock(
            location,
            LedgerUnlock.password,
            _bytes(_password),
            includeLedger: true,
          ),
        );
        final expense = (report['book']['recentEntries'] as List).singleWhere(
          (row) => row['kind'] == 'expense',
        );
        await _report(
          ledger.save(
            location.identity!,
            LedgerUnlock.password,
            _bytes(_password),
            {
              'kind': 'reviseEntry',
              'previousRevisionId': expense['revisionId'],
              'amountYuan': '20.00',
              'businessDate': '2026-01-07',
              'identity': {
                'eventId': expense['id'],
                'operationId': '1' * 32,
                'revisionId': '2' * 32,
              },
            },
          ),
        );
      });
      await save(result: '这笔流水已变化。请重新解锁并核对最新记录。');
      expect(find.text('账簿已打开'), findsNothing);
      await open();
      expect(find.text('合成银行 · 银行账户 · ¥2980.00'), findsOneWidget);
      await tap(find.text('编辑这笔支出'));
      expect(
        tester.widget<TextField>(field('金额（CNY）')).controller!.text,
        '20.00',
      );
      await enter('金额（CNY）', '25.00');
      await enter('业务日期（YYYY-MM-DD）', '2026-02-01');
      await save();
      await tap(find.widgetWithText(FilledButton, '关闭账簿'));
      await open();
      expect(find.text('合成银行 · 银行账户 · ¥2975.00'), findsOneWidget);
      expect(
        find.text('2026-02-01 · 支出 · ¥25.00\n合成银行 · 合成餐饮'),
        findsOneWidget,
      );
      await tap(find.text('编辑这笔收入'));
      await enter('金额（CNY）', '1900.00');
      await save();
      expect(find.text('合成银行 · 银行账户 · ¥2875.00'), findsOneWidget);
      expect(
        find.text('2026-01-05 · 收入 · ¥1900.00\n合成银行 · 合成工资'),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'delete confirmation binds the displayed revision and clears credentials on cancel',
    (tester) async {
      final directory = Directory.systemTemp.createTempSync(
        'radish_delete_widget_',
      );
      addTearDown(() => directory.deleteSync(recursive: true));
      final platform = UnavailableSystem(directory);
      late InternalLedger ledger;
      late LedgerIdentity identity;
      String id(int n) => n.toRadixString(16).padLeft(32, '0');
      Map<String, String> write(int operation, int event, int revision) => {
        'operationId': id(operation),
        'eventId': id(event),
        'revisionId': id(revision),
      };
      Future<void> save(Map<String, Object?> command) async {
        await _report(
          ledger.save(
            identity,
            LedgerUnlock.password,
            _bytes(_password),
            command,
          ),
        );
      }

      await tester.runAsync(() async {
        ledger = await InternalLedger.open(platform);
        final created = await ledger.create(_bytes(_password));
        try {
          identity = InternalLedger.identity(created.report);
          await _report(
            ledger.confirm(identity, Uint8List.fromList(created.secret)),
          );
        } finally {
          created.clear();
        }
        await save({
          'kind': 'account',
          'id': id(1),
          'accountKind': 'cash',
          'name': '现金账户',
          'amountYuan': '100.00',
          'businessDate': '2026-01-01',
          'identity': write(3, 4, 5),
        });
        await save({
          'kind': 'category',
          'id': id(2),
          'categoryKind': 'income',
          'name': '合成收入',
        });
        await save({
          'kind': 'entry',
          'direction': 'income',
          'accountId': id(1),
          'categoryId': id(2),
          'amountYuan': '50.00',
          'businessDate': '2026-02-01',
          'identity': write(6, 7, 8),
        });
      });

      Future<void> tap(String label) async {
        final target = find.text(label);
        await tester.ensureVisible(target);
        await tester.tap(target);
        await tester.pump();
      }

      Future<void> submit(String label, String password, String result) async {
        final field = find.byType(TextField);
        await tester.ensureVisible(field);
        await tester.enterText(field, password);
        await tester.pump();
        await tap(label);
        await _until(
          tester,
          () =>
              find.text(result).evaluate().isNotEmpty &&
              find.byType(LinearProgressIndicator).evaluate().isEmpty,
        );
      }

      await tester.pumpWidget(InternalLedgerApp(platform: platform));
      await _until(tester, () => find.text('解锁本地账簿').evaluate().isNotEmpty);
      await submit('打开账簿', _password, '账簿已打开');
      await tap('删除这笔收入');
      expect(
        find.text('原记录\n2026-02-01 · 收入 · ¥50.00\n现金账户 · 合成收入'),
        findsOneWidget,
      );
      expect(find.textContaining('余额将减少¥50.00'), findsOneWidget);
      await tester.enterText(find.byType(TextField), _password);
      await tap('取消删除');
      expect(find.text('现金账户 · 现金 · ¥150.00'), findsOneWidget);
      await tap('删除这笔收入');
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        isEmpty,
      );
      expect(
        tester
            .widget<FilledButton>(find.widgetWithText(FilledButton, '确认删除'))
            .onPressed,
        isNull,
      );

      await tester.runAsync(
        () => save({
          'kind': 'reviseEntry',
          'previousRevisionId': id(8),
          'businessDate': '2026-02-01',
          'amountYuan': '60.00',
          'identity': write(9, 7, 10),
        }),
      );
      await submit('确认删除', _password, '这笔流水已变化。请重新解锁并核对最新记录。');
      expect(find.text('账簿已打开'), findsNothing);
      await submit('打开账簿', _password, '账簿已打开');
      expect(find.text('现金账户 · 现金 · ¥160.00'), findsOneWidget);
      await tap('删除这笔收入');
      expect(find.textContaining('余额将减少¥60.00'), findsOneWidget);
      await submit(
        '确认删除',
        'wrong synthetic password',
        '口令或恢复材料不正确，也可能是封装损坏。原账簿已保留。',
      );
      await submit('确认删除', _password, '已删除这笔收支');
      expect(find.text('还没有收支记录。'), findsOneWidget);
      expect(find.text('现金账户 · 现金 · ¥100.00'), findsOneWidget);
      await tap('关闭账簿');
      await submit('打开账簿', _password, '最近核对的保存已提交。');
      expect(find.text('还没有收支记录。'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );
  testWidgets(
    'browses older ordinary entries and refreshes a changed history',
    (tester) async {
      final root = Directory.systemTemp.createTempSync(
        'radish_history_widget_',
      );
      addTearDown(() => root.deleteSync(recursive: true));
      final platform = UnavailableSystem(root);
      late InternalLedger ledger;
      late LedgerIdentity identity;
      String id(int n) => n.toRadixString(16).padLeft(32, '0');
      Map<String, String> write(int n) => {
        'operationId': id(n),
        'eventId': id(n + 1),
        'revisionId': id(n + 2),
      };
      await tester.runAsync(() async {
        ledger = await InternalLedger.open(platform);
        final created = await ledger.create(_bytes(_password));
        try {
          identity = InternalLedger.identity(created.report);
          Future<void> save(Map<String, Object?> cmd) async {
            await _report(
              ledger.save(
                identity,
                LedgerUnlock.recovery,
                Uint8List.fromList(created.secret),
                cmd,
              ),
            );
          }

          await expectLater(
            ledger.readEntries(
              identity,
              LedgerUnlock.recovery,
              Uint8List.fromList(created.secret),
              index: 1,
              sequence: 0,
            ),
            throwsA(_failed('recovery_unconfirmed')),
          );
          await _report(
            ledger.confirm(identity, Uint8List.fromList(created.secret)),
          );
          await save({
            'kind': 'account',
            'id': id(1),
            'accountKind': 'cash',
            'name': '合成现金',
            'amountYuan': '100',
            'businessDate': '2026-01-01',
            'identity': write(10),
          });
          await save({
            'kind': 'category',
            'id': id(2),
            'categoryKind': 'income',
            'name': '合成收入',
          });
          for (var i = 1; i <= 21; i++) {
            await save({
              'kind': 'entry',
              'direction': 'income',
              'accountId': id(1),
              'categoryId': id(2),
              'amountYuan': i.toString(),
              'businessDate': i == 1 ? '2026-01-31' : '2026-02-01',
              'identity': write(100 + i * 3),
            });
          }
        } finally {
          created.clear();
        }
      });
      Future<void> tap(String label) async {
        final control = find.text(label);
        await tester.ensureVisible(control);
        await tester.tap(control);
        await tester.pump();
      }

      Future<void> submit(String label, String password, String result) async {
        final field = find.byType(TextField);
        await tester.ensureVisible(field);
        await tester.enterText(field, password);
        await tester.pump();
        await tap(label);
        await _until(
          tester,
          () =>
              find.text(result).evaluate().isNotEmpty &&
              find.byType(LinearProgressIndicator).evaluate().isEmpty,
        );
      }

      const first = '完整流水 · 第 1 页';
      const second = '完整流水 · 第 2 页';
      const newest = '2026-02-01 · 收入 · ¥21.00\n合成现金 · 合成收入';
      const oldest = '2026-01-31 · 收入 · ¥1.00\n合成现金 · 合成收入';
      await tester.pumpWidget(InternalLedgerApp(platform: platform));
      await _until(tester, () => find.text('解锁本地账簿').evaluate().isNotEmpty);
      await submit('打开账簿', _password, '账簿已打开');
      await tap('查看完整流水');
      expect(find.text(first), findsOneWidget);
      expect(find.text(newest), findsOneWidget);
      expect(find.text(oldest), findsNothing);
      await tap('继续查看下一页');
      await tester.enterText(find.byType(TextField), _password);
      await tap('取消翻页');
      expect(find.text(newest), findsOneWidget);
      await tap('下一页');
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        isEmpty,
      );
      await submit(
        '读取第 2 页',
        'incorrect synthetic password',
        '口令或恢复材料不正确，也可能是封装损坏。原账簿已保留。',
      );
      await submit('读取第 2 页', _password, second);
      expect(find.text(oldest), findsOneWidget);
      expect(find.text('下一页'), findsNothing);
      expect(find.text('已到最后一页'), findsOneWidget);
      await tap('编辑这笔收入');
      await tap('取消录入');
      await tap('丢弃输入');
      expect(find.text(second), findsOneWidget);
      await tap('上一页');
      expect(find.text(newest), findsOneWidget);
      await tap('下一页');
      await tester.runAsync(
        () => _report(
          ledger.save(identity, LedgerUnlock.password, _bytes(_password), {
            'kind': 'deleteEntry',
            'previousRevisionId': id(105),
            'identity': {
              'eventId': id(104),
              'operationId': id(500),
              'revisionId': id(501),
            },
          }),
        ),
      );
      await submit('读取第 2 页', _password, '翻页期间流水已变化。请重新解锁，从第一页核对。');
      expect(find.text('账簿已打开'), findsNothing);
      await submit('打开账簿', _password, '账簿已打开');
      expect(find.text('合成现金 · 现金 · ¥330.00'), findsOneWidget);
      await tap('查看完整流水');
      expect(find.text('下一页'), findsNothing);
      expect(find.text('已到最后一页'), findsOneWidget);
      await tester.binding.handlePopRoute();
      await tester.pump();
      expect(find.text('查看完整流水'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  test(
    'monthly report keeps refund signs, zero categories and offsetting activity',
    () {
      final payload = <String, dynamic>{
        'month': '2026-02',
        'sequence': 3,
        'incomeMinor': '0',
        'grossExpenseMinor': '500',
        'refundMinor': '500',
        'netExpenseMinor': '0',
        'surplusMinor': '0',
        'categoryNames': <String, dynamic>{'a': '购物', 'b': '餐饮', 'c': '交通'},
        'expenseCategoriesMinor': <String, dynamic>{
          'a': '-500',
          'b': '500',
          'c': '0',
        },
      };
      MonthlySummary parse() =>
          MonthlySummary.fromReport({'monthSummary': payload}, '2026-02', 3);
      final summary = parse();
      expect(summary.hasActivity, isTrue);
      expect(summary.categories, [
        (name: '餐饮', minor: '500'),
        (name: '交通', minor: '0'),
        (name: '购物', minor: '-500'),
      ]);
      payload['refundMinor'] = 5.0;
      expect(parse, throwsA(_failed('invalid_response')));
    },
  );

  testWidgets(
    'monthly summary follows selected revisions across months and deletion',
    (tester) async {
      final root = Directory.systemTemp.createTempSync('radish_month_widget_');
      addTearDown(() => root.deleteSync(recursive: true));
      final platform = UnavailableSystem(root);
      late InternalLedger ledger;
      late LedgerIdentity identity;
      String id(int n) => n.toRadixString(16).padLeft(32, '0');
      Map<String, String> write(int operation, int event, int revision) => {
        'operationId': id(operation),
        'eventId': id(event),
        'revisionId': id(revision),
      };
      await tester.runAsync(() async {
        ledger = await InternalLedger.open(platform);
        final created = await ledger.create(_bytes(_password));
        try {
          identity = InternalLedger.identity(created.report);
          Future<void> save(Map<String, Object?> command) async {
            await _report(
              ledger.save(
                identity,
                LedgerUnlock.recovery,
                Uint8List.fromList(created.secret),
                command,
              ),
            );
          }

          await expectLater(
            ledger.readMonth(
              identity,
              LedgerUnlock.password,
              _bytes(_password),
              month: '2026-01',
              sequence: 0,
            ),
            throwsA(_failed('recovery_unconfirmed')),
          );
          await _report(
            ledger.confirm(identity, Uint8List.fromList(created.secret)),
          );
          await save({
            'kind': 'account',
            'id': id(1),
            'accountKind': 'cash',
            'name': '合成现金',
            'amountYuan': '100.00',
            'businessDate': '2026-01-01',
            'identity': write(10, 11, 12),
          });
          await save({
            'kind': 'category',
            'id': id(2),
            'categoryKind': 'expense',
            'name': '合成餐饮',
          });
          await save({
            'kind': 'entry',
            'direction': 'expense',
            'accountId': id(1),
            'categoryId': id(2),
            'amountYuan': '10.00',
            'businessDate': '2026-01-31',
            'identity': write(20, 21, 22),
          });
        } finally {
          created.clear();
        }
      });
      Future<void> tap(String label) async {
        final target = find.text(label);
        await tester.ensureVisible(target);
        await tester.tap(target);
        await tester.pump();
      }

      Future<void> submit(String label, String password, String state) async {
        final field = find.byWidgetPredicate(
          (w) => w is TextField && w.obscureText,
        );
        await tester.ensureVisible(field);
        await tester.enterText(field, password);
        await tester.pump();
        await tap(label);
        await _until(
          tester,
          () =>
              find.text(state).evaluate().isNotEmpty &&
              find.byType(LinearProgressIndicator).evaluate().isEmpty,
        );
      }

      Future<void> open() async => submit('打开账簿', _password, '账簿已打开');
      Future<void> read(String month) async =>
          submit('读取月汇总', _password, '$month 月汇总');
      await tester.pumpWidget(InternalLedgerApp(platform: platform));
      await _until(tester, () => find.text('解锁本地账簿').evaluate().isNotEmpty);
      await open();
      await tap('查看月汇总');
      await read('2026-01');
      expect(find.text('净支出 ¥10.00'), findsOneWidget);
      expect(find.text('合成餐饮 · ¥10.00'), findsOneWidget);
      expect(find.text('结余 -¥10.00'), findsOneWidget);
      await tap('下个月');
      await tap('取消查询');
      expect(find.text('2026-01 月汇总'), findsOneWidget);
      await tap('下个月');
      await submit(
        '读取月汇总',
        'incorrect synthetic password',
        '口令或恢复材料不正确，也可能是封装损坏。原账簿已保留。',
      );
      expect(find.text('这个月没有收支或退款。'), findsNothing);
      await read('2026-02');
      expect(find.text('这个月没有收支或退款。'), findsOneWidget);
      expect(find.text('合成餐饮 · ¥0.00'), findsOneWidget);
      await tap('返回账簿');
      await tap('查看月汇总');
      expect(find.text('2026-02 月汇总'), findsOneWidget);
      await tap('上个月');
      await tester.runAsync(
        () => _report(
          ledger.save(identity, LedgerUnlock.password, _bytes(_password), {
            'kind': 'reviseEntry',
            'previousRevisionId': id(22),
            'businessDate': '2026-02-01',
            'amountYuan': '25.00',
            'identity': write(30, 21, 32),
          }),
        ),
      );
      await submit('读取月汇总', _password, '查询期间账务已变化。请重新解锁核对月汇总。');
      expect(find.text('账簿已打开'), findsNothing);
      await open();
      expect(
        find.text('2026-02-01 · 支出 · ¥25.00\n合成现金 · 合成餐饮'),
        findsOneWidget,
      );
      await tap('查看月汇总');
      await read('2026-02');
      expect(find.text('净支出 ¥25.00'), findsOneWidget);
      expect(find.text('合成餐饮 · ¥25.00'), findsOneWidget);
      await tap('上个月');
      await read('2026-01');
      expect(find.text('这个月没有收支或退款。'), findsOneWidget);
      await tap('返回账簿');
      await tap('删除这笔支出');
      await submit('确认删除', _password, '已删除这笔收支');
      expect(find.text('合成现金 · 现金 · ¥100.00'), findsOneWidget);
      await tap('查看月汇总');
      final monthField = find.byWidgetPredicate(
        (w) => w is TextField && w.decoration?.labelText == '业务月份（YYYY-MM）',
      );
      await tester.enterText(monthField, '2026-13');
      await submit('读取月汇总', _password, '统计月份无效');
      await tester.enterText(monthField, '2026-02');
      await read('2026-02');
      expect(find.text('净支出 ¥0.00'), findsOneWidget);
      expect(find.text('这个月没有收支或退款。'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );
}
