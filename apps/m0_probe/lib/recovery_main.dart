// M0 专用恢复 runner；命令文件只含合成 run / 中断点，不接收秘密。
import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'src/probe.dart';
import 'src/validation.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(
    const MaterialApp(
      home: Scaffold(body: Center(child: Text('合成跨进程恢复实验正在运行'))),
    ),
  );
  final root = Directory.systemTemp.path;
  // 仅输出实验目录元数据，供 host 核对平台实际路径；不含账务或秘密。
  debugPrint('M0_RECOVERY_DIRECTORY=$root');
  if (!await File('$root/m0-recovery-command.json').exists()) {
    runApp(
      const MaterialApp(
        home: Scaffold(body: Center(child: Text('等待合成恢复实验命令'))),
      ),
    );
    return;
  }
  Map<String, dynamic> result;
  try {
    final input = await File('$root/m0-recovery-command.json').readAsString();
    if (input.length > 4096) throw const FormatException('实验命令过长');
    final command = jsonDecode(input) as Map<String, dynamic>;
    final candidate = BridgeCandidate.values.byName(
      command['bridge'] as String,
    );
    result = await callValidation(
      candidate,
      command['action'] as Map<String, dynamic>,
      directory: root,
    );
    result['runnerPid'] = pid;
  } catch (error) {
    result = {'ok': false, 'runnerError': error.toString(), 'runnerPid': pid};
  }
  final temporary = File('$root/m0-recovery-result.json.tmp');
  await temporary.writeAsString(jsonEncode(result), flush: true);
  await temporary.rename('$root/m0-recovery-result.json');
  runApp(
    MaterialApp(
      home: Scaffold(
        body: Center(
          child: Text(result['ok'] == true ? '合成恢复核对通过' : '合成恢复实验失败'),
        ),
      ),
    ),
  );
}
