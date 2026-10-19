// M0-C 专用合成进程实验。FIFO 不落盘秘密，控制报告只输出非秘密状态。
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'package:flutter/material.dart';
import 'bootstrap.dart';
import 'probe.dart';

Future<void> _readExact(RandomAccessFile file, List<int> bytes) async {
  var offset = 0;
  while (offset < bytes.length) {
    final count = await file.readInto(bytes, offset);
    if (count == 0) throw const BootstrapFailure('control_closed');
    offset += count;
  }
}

Future<(Map<String, dynamic>, Uint8List)> _receive(String root) async {
  final file = await File('$root/control-in').open();
  Uint8List? secret;
  try {
    final header = Uint8List(8);
    await _readExact(file, header);
    final data = ByteData.sublistView(header);
    final metadataLength = data.getUint32(0);
    final secretLength = data.getUint32(4);
    if (metadataLength == 0 || metadataLength > 4096 || secretLength > 1104) {
      throw const BootstrapFailure('invalid_control');
    }
    final metadata = Uint8List(metadataLength);
    secret = Uint8List(secretLength);
    await _readExact(file, metadata);
    await _readExact(file, secret);
    return (jsonDecode(utf8.decode(metadata)) as Map<String, dynamic>, secret);
  } catch (_) {
    secret?.fillRange(0, secret.length, 0);
    rethrow;
  } finally {
    await file.close();
  }
}

Future<void> _respond(String root, BootstrapResult result) async {
  final metadata = utf8.encode(jsonEncode(result.status));
  if (metadata.length > 16384 || result.secret.length > 128) {
    throw const BootstrapFailure('invalid_response');
  }
  final header = ByteData(8)
    ..setUint32(0, metadata.length)
    ..setUint32(4, result.secret.length);
  // writeOnly 不截断普通文件；平台先以 lstat 确认该路径为本次 FIFO。
  final output = await File('$root/control-out').open(mode: FileMode.writeOnly);
  try {
    await output.writeFrom(header.buffer.asUint8List());
    await output.writeFrom(metadata);
    await output.writeFrom(result.secret);
  } finally {
    await output.close();
  }
}

Future<BootstrapResult> _dispatch(
  SystemProtection platform,
  String root,
  Map<String, dynamic> command,
  Uint8List input,
) async {
  final bridge = BridgeCandidate.values.byName(command['bridge'] as String);
  final run = command['run'] as String;
  if (!RegExp(r'^[0-9a-f]{32}$').hasMatch(run)) {
    throw const BootstrapFailure('invalid_control');
  }
  final service = BootstrapService(root, run, bridge, platform);
  switch (command['op']) {
    case 'core':
      return service.core(
        Map<String, Object?>.from(command['action'] as Map),
        input,
        pauseAt: command['pauseAt'] as String?,
        fileFault: command['fileFault'] == null
            ? null
            : Map<String, String>.from(command['fileFault'] as Map),
      );
    case 'capabilities':
      return BootstrapResult({
        'ok': true,
        'report': await platform.capabilities(),
      }, Uint8List(0));
    case 'establish':
      return service.establishProtection(command['unlock'] as String, input);
    case 'unlockSystem':
      return service.unlockSystem();
    case 'createProtection':
      return BootstrapResult({
        'ok': true,
        'report': await platform.create(
          run,
          command['generation'] as String,
          input,
        ),
      }, Uint8List(0));
    case 'readProtection':
      return BootstrapResult({
        'ok': true,
        'report': {'read': true},
      }, await platform.read(run, command['generation'] as String));
    case 'deleteProtection':
      return BootstrapResult({
        'ok': true,
        'report': await platform.delete(run, command['generation'] as String),
      }, Uint8List(0));
    case 'cleanup':
      final status = await service.core({'kind': 'status'}, Uint8List(0));
      status.requireSuccess();
      final candidates = List<String>.from(status.report['candidates'] as List);
      for (final generation in candidates) {
        await platform.delete(run, generation);
      }
      status.clear();
      return service.core({'kind': 'cleanup'}, Uint8List(0));
    case 'stop':
      return BootstrapResult({
        'ok': true,
        'report': {'stopped': true},
      }, Uint8List(0));
    default:
      throw const BootstrapFailure('invalid_control');
  }
}

Future<void> runBootstrapControl() async {
  final status = ValueNotifier<String>('正在准备合成解锁实验');
  runApp(
    MaterialApp(
      home: Scaffold(
        body: SafeArea(
          child: Center(
            child: ValueListenableBuilder<String>(
              valueListenable: status,
              builder: (context, text, child) =>
                  Text(text, textAlign: TextAlign.center),
            ),
          ),
        ),
      ),
    ),
  );
  final platform = SystemProtection();
  var stage = 'initialize';
  try {
    final root = await platform.control();
    await File('$root/control-ready.json').writeAsString(
      jsonEncode({
        'pid': pid,
        'directory': root,
        'platform': Platform.operatingSystem,
        'capabilities': await platform.capabilities(),
      }),
      flush: true,
    );
    debugPrint('M0C_CONTROL_DIRECTORY=$root');
    while (true) {
      stage = 'receive';
      status.value = '等待合成实验命令\n秘密仅经受限管道传递';
      final (command, input) = await _receive(root);
      stage = 'dispatch';
      status.value = '正在核对原账库与未同步事实';
      var ticks = 0;
      var peakRss = ProcessInfo.currentRss;
      final timer = Timer.periodic(const Duration(milliseconds: 20), (_) {
        ticks++;
        final rss = ProcessInfo.currentRss;
        if (rss > peakRss) peakRss = rss;
      });
      final watch = Stopwatch()..start();
      BootstrapResult? result;
      try {
        result = await _dispatch(platform, root, command, input);
      } on BootstrapFailure catch (error) {
        result = BootstrapResult({
          'ok': false,
          'error': {'code': error.code, 'platformCode': error.platformCode},
        }, Uint8List(0));
      } catch (_) {
        result = BootstrapResult({
          'ok': false,
          'error': {'code': 'control_error'},
        }, Uint8List(0));
      } finally {
        timer.cancel();
        watch.stop();
        input.fillRange(0, input.length, 0);
      }
      result.status['measurement'] = {
        'elapsedMs': watch.elapsedMilliseconds,
        'sampledPeakRss': peakRss,
        'processMaxRss': ProcessInfo.maxRss,
        'heartbeatTicks': ticks,
        'pid': pid,
      };
      try {
        stage = 'respond';
        await _respond(root, result);
      } finally {
        stage = 'clear';
        result.clear();
      }
      if (command['op'] == 'stop') {
        await platform.control(remove: true);
        status.value = '合成实验已结束，控制管道已清理';
        return;
      }
    }
  } catch (error) {
    status.value = '合成控制通道已停止，请检查非秘密诊断状态';
    debugPrint('M0C_CONTROL_STOPPED:$stage:${error.runtimeType}');
  }
}
