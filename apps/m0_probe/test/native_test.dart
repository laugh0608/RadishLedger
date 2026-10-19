import 'dart:convert';
import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/native.dart';
import 'package:radishledger_probe/src/probe.dart';

void main() {
  test(
    'thin FFI executes actual encrypted core with a Unicode temporary path',
    () {
      final directory = Directory.systemTemp.createTempSync('radishledger_合成_');
      try {
        final value =
            jsonDecode(runThin(directory.path)) as Map<String, Object?>;
        expect(value['ok'], true);
        final report = ProbeReport(value['report']! as Map<String, Object?>);
        expect((report.caseCount, report.checkpointCount), (11, 40));
        expect(directory.listSync(), isEmpty);
        expect(File(loadedLibraryPath()).existsSync(), true);
      } finally {
        directory.deleteSync();
      }
    },
  );
  test('invalid input returns a persistent structured core error', () {
    final value = jsonDecode(runThin('relative-path')) as Map<String, Object?>;
    expect(value['ok'], false);
    expect(
      (value['error']! as Map<String, Object?>)['code'],
      'invalid_directory',
    );
  });
  test('FRB candidate executes the same actual core', () async {
    final report = await runProbe(BridgeCandidate.frb);
    expect((report.caseCount, report.checkpointCount), (11, 40));
  });
}
