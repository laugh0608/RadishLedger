import 'dart:convert';
import 'dart:io';
import 'package:integration_test/integration_test_driver_extended.dart';

Future<void> main() async {
  await integrationDriver(
    responseDataCallback: (data) async {
      final platform = data?['platform'];
      if (data?['schema'] != 1 ||
          !['ios', 'android'].contains(platform) ||
          data?['resilience'] is! Map) {
        throw const FormatException('缺少目标平台压力结果');
      }
      final evidence = Directory('../../local-evidence/m0-b');
      await evidence.create(recursive: true);
      await File(
        '${evidence.path}/$platform-resilience-report.json',
      ).writeAsString('${const JsonEncoder.withIndent('  ').convert(data)}\n');
    },
  );
}
