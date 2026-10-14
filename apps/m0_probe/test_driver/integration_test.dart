import 'dart:convert';
import 'dart:io';
import 'package:integration_test/integration_test_driver_extended.dart';

Future<void> main() async {
  final evidence = Directory('../../local-evidence/m0-b');
  await evidence.create(recursive: true);
  await integrationDriver(
    onScreenshot: (name, bytes, [arguments]) async {
      if (!RegExp(r'^(ios|android)-(thin|frb)$').hasMatch(name) ||
          bytes.isEmpty) {
        return false;
      }
      await File('${evidence.path}/$name.png').writeAsBytes(bytes);
      return true;
    },
    responseDataCallback: (data) async {
      final thin = data?['thin'] as Map<String, dynamic>?;
      final frb = data?['frb'] as Map<String, dynamic>?;
      final target = thin?['nativeTarget'];
      if (thin == null || frb == null || frb['nativeTarget'] != target) {
        throw const FormatException('没有取得两种 bridge 的同平台实际报告');
      }
      final platform = switch (target) {
        'aarch64-ios' => 'ios',
        'aarch64-android' => 'android',
        _ => throw const FormatException('报告不是本批移动 arm64 目标'),
      };
      await File(
        '${evidence.path}/$platform-simulator-report.json',
      ).writeAsString(
        '${const JsonEncoder.withIndent('  ').convert({'thin': thin, 'frb': frb})}\n',
      );
    },
  );
}
