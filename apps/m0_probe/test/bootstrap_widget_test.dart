import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/bootstrap_app.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/bootstrap_selection.dart';
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

class CleanupProtection extends TestProtection {
  final deleted = <String>[];
  @override
  Future<Map<String, dynamic>> delete(String run, String generation) async {
    deleted.add('$run.$generation');
    return {'deleted': true};
  }
}

class CleanupService extends BootstrapService {
  CleanupService(String directory, CleanupProtection platform)
    : super(
        directory,
        '11111111111111111111111111111111',
        BridgeCandidate.thin,
        platform,
      );
  static const generation = '22222222222222222222222222222222';
  bool failCleanup = true;
  @override
  Future<BootstrapResult> core(
    Map<String, Object?> action,
    Uint8List input, {
    String? pauseAt,
    Map<String, String>? fileFault,
  }) async {
    input.fillRange(0, input.length, 0);
    if (action['kind'] == 'status') {
      return BootstrapResult({
        'ok': true,
        'report': {
          'activeProtection': generation,
          'candidates': [generation],
        },
      }, Uint8List(0));
    }
    if (action['kind'] == 'cleanup') {
      if (failCleanup) throw const BootstrapFailure('bootstrap_io');
      await Directory('$directory/m0c-$run').delete();
      return BootstrapResult({
        'ok': true,
        'report': {'cleaned': true},
      }, Uint8List(0));
    }
    throw StateError('unexpected core action');
  }
}

Future<void> mount(WidgetTester tester, BootstrapService? service) async {
  await tester.pumpWidget(BootstrapApp(service: service));
  await tester.runAsync(() async {
    final deadline = DateTime.now().add(const Duration(seconds: 5));
    do {
      await Future<void>.delayed(const Duration(milliseconds: 5));
      await tester.pump();
      if (find.text('正在核对实验目录').evaluate().isEmpty) return;
    } while (DateTime.now().isBefore(deadline));
    fail('UI did not finish bounded selection initialization');
  });
  await tester.pumpAndSettle();
}

Future<void> waitForIo(WidgetTester tester) async {
  await tester.runAsync(() async {
    final deadline = DateTime.now().add(const Duration(seconds: 5));
    do {
      await Future<void>.delayed(const Duration(milliseconds: 5));
      await tester.pump();
      if (find.byType(LinearProgressIndicator).evaluate().isEmpty) return;
    } while (DateTime.now().isBefore(deadline));
    fail('UI did not finish bounded experiment IO');
  });
  await tester.pumpAndSettle();
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
    Map<String, String>? fileFault,
  }) {
    calls++;
    input.fillRange(0, input.length, 0);
    return pending.future;
  }
}

void main() {
  testWidgets('invalid create input leaves no selection or core files', (
    tester,
  ) async {
    final directory = await tester.runAsync(
      () => Directory.systemTemp.createTemp('radishledger_create_ui_'),
    );
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(
      SystemProtection.channel,
      (call) async => switch (call.method) {
        'storage' => directory!.path,
        'capabilities' => {'supported': false},
        'cancel' => null,
        _ => throw StateError('unexpected platform operation'),
      },
    );
    try {
      await mount(tester, null);
      final create = find.byKey(const ValueKey('m0c-create'));
      final password = find.byKey(const ValueKey('m0c-password'));
      for (final input in ['', List.filled(1025, 'x').join()]) {
        await tester.ensureVisible(password);
        await tester.enterText(password, input);
        await tester.ensureVisible(create);
        await tester.tap(create);
        await tester.pumpAndSettle();
        expect(find.text('请输入有效口令。'), findsOneWidget);
        expect(find.byKey(const ValueKey('m0c-create')), findsOneWidget);
        expect(await tester.runAsync(() => directory!.list().isEmpty), isTrue);
      }
    } finally {
      await tester.pumpWidget(const SizedBox());
      messenger.setMockMethodCallHandler(SystemProtection.channel, null);
      await tester.runAsync(() => directory!.delete());
    }
  });

  testWidgets(
    'cancel suppresses late unlock and keeps input disabled until completion',
    (tester) async {
      final platform = TestProtection();
      final service = ControlledService(platform);
      await mount(tester, service);
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
    await mount(tester, service);
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

  testWidgets(
    'cleanup cancel preserves data; failed cleanup survives page restart and permits bridge switch only after success',
    (tester) async {
      final directory = await tester.runAsync(
        () => Directory.systemTemp.createTemp('radishledger_cleanup_ui_'),
      );
      final platform = CleanupProtection();
      final service = CleanupService(directory!.path, platform);
      await tester.runAsync(() async {
        await Directory('${directory.path}/m0c-${service.run}').create();
        await BootstrapSelection(
          service.run,
          service.bridge,
        ).save(directory.path, create: true);
      });
      await mount(tester, service);
      final cleanup = find.byKey(const ValueKey('m0c-cleanup'));
      await tester.ensureVisible(cleanup);
      await tester.tap(cleanup);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(find.textContaining('未同步操作'), findsOneWidget);
      await tester.tap(find.text('取消'));
      await waitForIo(tester);
      expect(platform.deleted, isEmpty);
      expect(find.byKey(const ValueKey('m0c-unlock-password')), findsOneWidget);

      await tester.ensureVisible(cleanup);
      await tester.tap(cleanup);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      await tester.tap(find.text('确认本次操作'));
      await waitForIo(tester);
      expect(find.text('重试本次清理'), findsOneWidget);
      expect(find.byKey(const ValueKey('m0c-unlock-password')), findsNothing);
      expect(
        tester
            .widget<DropdownButtonFormField<BridgeCandidate>>(
              find.byType(DropdownButtonFormField<BridgeCandidate>),
            )
            .onChanged,
        isNull,
      );
      expect(
        await tester.runAsync(
          () async => (await BootstrapSelection.load(
            directory.path,
          ))!.cleanupCandidates,
        ),
        [CleanupService.generation],
      );

      await tester.pumpWidget(const SizedBox());
      final restartedPlatform = CleanupProtection();
      final restarted = CleanupService(directory.path, restartedPlatform)
        ..failCleanup = false;
      await mount(tester, restarted);
      expect(find.textContaining('上次清理尚未完成'), findsOneWidget);
      await tester.ensureVisible(cleanup);
      await tester.tap(cleanup);
      await waitForIo(tester);
      expect(find.byKey(const ValueKey('m0c-create')), findsOneWidget);
      expect(find.byKey(const ValueKey('m0c-cleanup')), findsNothing);
      expect(find.textContaining('可以选择另一接入候选'), findsOneWidget);
      final dropdown = find.byType(DropdownButtonFormField<BridgeCandidate>);
      expect(
        tester
            .widget<DropdownButtonFormField<BridgeCandidate>>(dropdown)
            .onChanged,
        isNotNull,
      );
      await tester.ensureVisible(dropdown);
      await tester.tap(dropdown);
      await tester.pumpAndSettle();
      await tester.tap(find.text('frb').last);
      await tester.pumpAndSettle();
      expect(
        tester.state<FormFieldState<BridgeCandidate>>(dropdown).value,
        BridgeCandidate.frb,
      );
      expect(await tester.runAsync(() => directory.list().isEmpty), isTrue);
      await tester.pumpWidget(const SizedBox());
      await tester.runAsync(directory.delete);
    },
  );

  testWidgets(
    'exact item deletion requires confirmation and background cancels pending consent',
    (tester) async {
      final platform = CleanupProtection();
      final service = CleanupService('/synthetic', platform);
      await mount(tester, service);
      final remove = find.byKey(const ValueKey('m0c-remove-protection'));
      await tester.ensureVisible(remove);
      await tester.tap(remove);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      expect(find.textContaining(CleanupService.generation), findsOneWidget);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      await tester.tap(find.text('确认本次操作'));
      await tester.pumpAndSettle();
      expect(platform.deleted, isEmpty);
      await tester.ensureVisible(remove);
      await tester.tap(remove);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      await tester.tap(find.text('确认本次操作'));
      await tester.pumpAndSettle();
      expect(platform.deleted, ['${service.run}.${CleanupService.generation}']);
      expect(find.textContaining('再用独立材料读取原账库'), findsOneWidget);
    },
  );
}
