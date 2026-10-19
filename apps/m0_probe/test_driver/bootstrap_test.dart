import 'dart:convert';
import 'dart:io';
import 'package:integration_test/integration_test_driver_extended.dart';

Future<void> main() async {
  final subdirectory = Platform.environment['M0C_EVIDENCE_SUBDIR'];
  if (subdirectory != null &&
      !RegExp(r'^[a-zA-Z0-9_-]+$').hasMatch(subdirectory)) {
    throw const FormatException('证据子目录必须是单个明确名称');
  }
  final evidence = Directory(
    '../../local-evidence/m0-c${subdirectory == null ? '' : '/$subdirectory'}',
  );
  await evidence.create(recursive: true);
  await integrationDriver(
    onScreenshot: (name, bytes, [arguments]) async {
      if (!RegExp(r'^(ios|android)-(thin|frb)-bootstrap$').hasMatch(name) ||
          bytes.isEmpty) {
        return false;
      }
      await File('${evidence.path}/$name.png').writeAsBytes(bytes);
      return true;
    },
    responseDataCallback: (data) async {
      final platform = data?['platform'];
      if (!['ios', 'android'].contains(platform) ||
          data?['thin'] == null ||
          data?['frb'] == null) {
        throw const FormatException('双桥接 UI 结果不完整');
      }
      final summary = <String, dynamic>{
        'platform': platform,
        'thin': data!['thin'],
        'frb': data['frb'],
      };
      await File('${evidence.path}/$platform-bootstrap-ui.json').writeAsString(
        '${const JsonEncoder.withIndent('  ').convert(summary)}\n',
      );
    },
  );
}
