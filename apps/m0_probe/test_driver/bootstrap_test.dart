import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'package:integration_test/integration_test_driver_extended.dart';

Future<void> main() async {
  final subdirectory = Platform.environment['M0C_EVIDENCE_SUBDIR'];
  final session = Platform.environment['M0C_EVIDENCE_SESSION'];
  final platform = Platform.environment['M0C_UI_PLATFORM'];
  final device = Platform.environment['M0C_UI_DEVICE'];
  if (subdirectory == null ||
      !RegExp(r'^[a-zA-Z0-9_-]+$').hasMatch(subdirectory) ||
      session == null ||
      !RegExp(r'^[0-9a-f]{32}$').hasMatch(session) ||
      !['ios', 'android'].contains(platform) ||
      device == null ||
      device.isEmpty) {
    throw const FormatException('必须指定本批证据目录、身份、平台和确切模拟器');
  }
  final evidence = Directory('../../local-evidence/m0-c/$subdirectory');
  await evidence.create(recursive: true);
  final monitor = await Process.start('python3', [
    '-B',
    '../../scripts/observe-bootstrap-ui-background.py',
    '--platform',
    platform!,
    '--device',
    device,
    '--session',
    session,
  ]);
  final monitorOutput = monitor.stdout.transform(utf8.decoder).join();
  final monitorErrors = monitor.stderr.transform(utf8.decoder).join();
  var stopped = false;
  Future<void> stopMonitor({required bool requireSuccess}) async {
    if (stopped) return;
    stopped = true;
    int code;
    try {
      code = await monitor.exitCode.timeout(const Duration(seconds: 5));
    } on TimeoutException {
      monitor.kill();
      code = await monitor.exitCode.timeout(const Duration(seconds: 35));
    }
    await monitorOutput;
    await monitorErrors;
    if (requireSuccess && code != 0) {
      throw StateError('本批后台控制未完成（exit=$code），不能判定移动生命周期通过');
    }
  }

  try {
    await integrationDriver(
      writeResponseOnFailure: true,
      onScreenshot: (name, bytes, [arguments]) async {
        if (!RegExp(
              r'^(ios|android)-(thin|frb)-bootstrap-(review|large-text|cleanup-retry)$',
            ).hasMatch(name) ||
            bytes.isEmpty) {
          return false;
        }
        final file = File('${evidence.path}/$name.png');
        await file.create(exclusive: true);
        await file.writeAsBytes(bytes);
        return true;
      },
      responseDataCallback: (data) async {
        try {
          final completed = data?['completed'] == true;
          await stopMonitor(requireSuccess: completed);
          if (data?['platform'] != platform || data?['session'] != session) {
            throw const FormatException('本批 UI 报告身份或平台不完整');
          }
          final file = File('${evidence.path}/$platform-bootstrap-ui.json');
          await file.create(exclusive: true);
          await file.writeAsString(
            '${const JsonEncoder.withIndent('  ').convert({'platform': platform, 'session': session, 'completed': completed, 'thin': data?['thin'], 'frb': data?['frb'], 'ownedRuns': data?['ownedRuns'], 'cleanupComplete': data?['cleanupComplete'], 'systemProtection': data?['systemProtection']})}\n',
          );
          if (!completed ||
              data?['thin'] == null ||
              data?['frb'] == null ||
              data?['cleanupComplete'] != true) {
            throw const FormatException('双桥接 UI 流程或精确清理未完成');
          }
        } finally {
          await stopMonitor(requireSuccess: false);
        }
      },
    );
  } finally {
    // integrationDriver 正常完成会 exit；正常 / 测试失败由上方 callback 收尾。
    // 连接、截图或报告异常仍在此回收本批 monitor。
    await stopMonitor(requireSuccess: false);
  }
}
