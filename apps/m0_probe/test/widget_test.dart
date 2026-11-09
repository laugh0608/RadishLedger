import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/probe_app.dart';
import 'package:radishledger_probe/src/probe.dart';

void main() {
  testWidgets('UI failure stays visible and can be retried', (tester) async {
    var calls = 0;
    await tester.pumpWidget(
      ProbeApp(
        run: (_) async {
          calls++;
          throw StateError('合成核心不可用');
        },
      ),
    );
    await tester.tap(find.byKey(const ValueKey('run-probe')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('probe-error')), findsOneWidget);
    expect(find.byKey(const ValueKey('probe-success')), findsNothing);
    await tester.tap(find.byKey(const ValueKey('run-probe')));
    await tester.pumpAndSettle();
    expect(calls, 2);
  });

  testWidgets(
    'stopping UI wait does not claim a core rollback or allow overlap',
    (tester) async {
      final pending = Completer<ProbeReport>();
      var calls = 0;
      await tester.pumpWidget(
        ProbeApp(
          run: (_) {
            calls++;
            return pending.future;
          },
        ),
      );
      await tester.tap(find.byKey(const ValueKey('run-probe')));
      await tester.pump();
      await tester.tap(find.byKey(const ValueKey('run-probe')));
      await tester.tap(find.byKey(const ValueKey('stop-waiting')));
      await tester.pump();
      expect(calls, 1);
      expect(find.textContaining('核心仍在完成'), findsOneWidget);
      pending.completeError(StateError('合成实验已终止'));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('probe-success')), findsNothing);
      expect(find.text('当前实验已结束，可以重新运行。'), findsOneWidget);
    },
  );
}
