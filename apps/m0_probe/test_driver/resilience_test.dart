import 'dart:convert';
import 'dart:io';
import 'package:integration_test/integration_test_driver_extended.dart';

Future<void> main() async {
  final subdirectory = Platform.environment['M0B_EVIDENCE_SUBDIR'];
  if (subdirectory != null &&
      !RegExp(r'^[a-zA-Z0-9_-]+$').hasMatch(subdirectory)) {
    throw const FormatException('证据子目录必须是单个明确名称');
  }
  await integrationDriver(
    responseDataCallback: (data) async {
      final platform = data?['platform'];
      if (data?['schema'] != 1 ||
          !['ios', 'android'].contains(platform) ||
          data?['resilience'] is! Map) {
        throw const FormatException('缺少目标平台压力结果');
      }
      final evidence = Directory(
        '../../local-evidence/m0-b${subdirectory == null ? '' : '/$subdirectory'}',
      );
      await evidence.create(recursive: true);
      await File(
        '${evidence.path}/$platform-resilience-report.json',
      ).writeAsString('${const JsonEncoder.withIndent('  ').convert(data)}\n');
    },
  );
}
