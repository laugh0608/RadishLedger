import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'frb_generated/api.dart' as frb;
import 'frb_generated/frb_generated.dart';
import 'native.dart';
import 'probe.dart';

Future<T> _withBridge<T>(
  BridgeCandidate candidate,
  Future<T> Function(Future<String> Function(String)) action,
) async {
  if (candidate == BridgeCandidate.thin) {
    return action((request) async => runThinValidation(request));
  }
  await RustLib.init(
    externalLibrary: ExternalLibrary.open(loadedLibraryPath()),
  );
  try {
    return await action((request) => frb.runValidation(requestJson: request));
  } finally {
    RustLib.dispose();
  }
}

String _request(Map<String, Object?> action, String directory, int abi) =>
    jsonEncode({'abi': abi, 'directory': directory, 'action': action});

Future<Map<String, dynamic>> callValidation(
  BridgeCandidate candidate,
  Map<String, Object?> action, {
  String? directory,
  int abi = 1,
}) => Isolate.run(
  () => _withBridge(
    candidate,
    (call) async =>
        jsonDecode(
              await call(
                _request(action, directory ?? Directory.systemTemp.path, abi),
              ),
            )
            as Map<String, dynamic>,
  ),
);

void _expectError(Map<String, dynamic> value, String code) {
  if (value['ok'] != false ||
      value['error'] is! Map ||
      value['error']['code'] != code) {
    throw StateError('实际 native 错误未按契约返回：$code');
  }
}

/// 有界压力，不把 RSS 采样宣称为完整泄漏检测或真机资源结论。
Future<Map<String, Object?>> runBridgePressure(BridgeCandidate candidate) =>
    Isolate.run(() async {
      if (candidate == BridgeCandidate.thin) verifyNativeBufferBounds();
      return _withBridge(candidate, (call) async {
        final directory = Directory.systemTemp.path;
        for (final pair in [
          ('panic', 'internal_error'),
          ('nativeError', 'synthetic_native_error'),
        ]) {
          _expectError(
            jsonDecode(await call(_request({'kind': pair.$1}, directory, 1)))
                as Map<String, dynamic>,
            pair.$2,
          );
        }
        _expectError(
          jsonDecode(await call(_request({'kind': 'panic'}, directory, 99)))
              as Map<String, dynamic>,
          'abi_mismatch',
        );
        final request = _request({'kind': 'nativeError'}, directory, 1);
        for (var i = 0; i < 200; i++) {
          _expectError(
            jsonDecode(await call(request)) as Map<String, dynamic>,
            'synthetic_native_error',
          );
        }
        final samples = <int>[ProcessInfo.currentRss];
        for (var batch = 0; batch < 3; batch++) {
          for (var i = 0; i < 2000; i++) {
            _expectError(
              jsonDecode(await call(request)) as Map<String, dynamic>,
              'synthetic_native_error',
            );
          }
          if (nativeBufferCount() != 0) throw StateError('压力批次后句柄未归零');
          samples.add(ProcessInfo.currentRss);
        }
        // 单次 debug 进程中的保守异常增长门槛；原始样本保留供审阅。
        if (samples.last - samples.first > 64 * 1024 * 1024) {
          throw StateError('有界压力出现超过 64 MiB 的 RSS 增长');
        }
        final concurrent =
            jsonDecode(
                  await call(_request({'kind': 'concurrent'}, directory, 1)),
                )
                as Map<String, dynamic>;
        if (concurrent['ok'] != true) {
          throw StateError('panic 后并发核心不可用：$concurrent');
        }
        return {
          'calls': 6200,
          'panicCaught': true,
          'abiRejected': true,
          'nativeErrorRejected': true,
          'liveBuffers': nativeBufferCount(),
          'rssBytes': samples,
          'concurrent': concurrent['report'],
        };
      });
    });
