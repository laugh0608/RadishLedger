import 'dart:convert';
import 'dart:io';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/archive_download.dart';
import 'package:radishledger_probe/src/bootstrap.dart';
import 'package:radishledger_probe/src/internal_ledger.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('radishledger/internal/archive');
  test('下载回读认证、取消和文件变化分别决定能否发布', () async {
    final root = await Directory.systemTemp.createTemp('rl_archive_');
    addTearDown(() => root.delete(recursive: true));
    final ledger = InternalLedger(root.path, const SystemProtection());
    Uint8List bytes(String s) => Uint8List.fromList(utf8.encode(s));
    final created = await ledger.create(bytes('synthetic local'));
    final identity = InternalLedger.identity(created.report);
    final ready = await ledger.confirm(
      identity,
      Uint8List.fromList(created.secret),
    );
    ready.clear();
    created.clear();

    var active = true;
    var cancel = false;
    var corrupt = false;
    final calls = <String>[];
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call.method);
      if (call.method == 'prepare') {
        final stage = Directory('${root.path}/${call.arguments['stage']}');
        final cipher = await File('${stage.path}/sealed').readAsBytes();
        if (corrupt) cipher.last ^= 1;
        await File('${stage.path}/readback').writeAsBytes(cipher);
        if (cancel) active = false;
      }
      if (call.method == 'publish') return 'test.rla';
      return null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));

    Future<BootstrapResult> export() async {
      calls.clear();
      active = true;
      final local = bytes('synthetic local');
      final pass = bytes('synthetic archive');
      try {
        return await exportArchive(
          directory: root.path,
          run: InternalLedger.run,
          identity: identity,
          unlock: 'password',
          name: 'test',
          local: local,
          password: pass,
          active: () => active,
        );
      } finally {
        expect(local.every((b) => b == 0) && pass.every((b) => b == 0), true);
        expect(
          await root
              .list()
              .map((entity) => entity.path.split('/').last)
              .toList(),
          ['m0c-${InternalLedger.run}'],
        );
      }
    }

    final result = await export();
    expect(result.report['archiveName'], 'test.rla');
    result.clear();
    expect(calls, ['prepare', 'publish']);
    cancel = true;
    await expectLater(
      export(),
      throwsA(
        isA<BootstrapFailure>().having((e) => e.code, 'code', 'cancelled'),
      ),
    );
    expect(calls, ['prepare', 'discard']);
    cancel = false;
    corrupt = true;
    await expectLater(
      export(),
      throwsA(
        isA<BootstrapFailure>().having(
          (e) => e.code,
          'code',
          'archive_file_mismatch',
        ),
      ),
    );
    expect(calls, ['prepare', 'discard']);
  });
}
