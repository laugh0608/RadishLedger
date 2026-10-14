import 'dart:convert';
import 'dart:isolate';
import 'package:flutter/services.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'frb_generated/api.dart' as frb;
import 'frb_generated/frb_generated.dart';
import 'native.dart';
import 'probe.dart';

class BootstrapFailure implements Exception {
  const BootstrapFailure(this.code, [this.platformCode]);
  final String code;
  final Object? platformCode;
  @override
  String toString() => 'BootstrapFailure($code)';
}

class BootstrapResult {
  BootstrapResult(this.status, this.secret);
  final Map<String, dynamic> status;
  final Uint8List secret;
  Map<String, dynamic> get report => status['report'] as Map<String, dynamic>;
  void clear() => secret.fillRange(0, secret.length, 0);
  void requireSuccess() {
    if (status['ok'] != true) {
      clear();
      throw BootstrapFailure(
        (status['error'] as Map?)?['code'] as String? ?? 'invalid_response',
      );
    }
  }
}

/// 接管并清理 input；跨 isolate / FRB codec 的内部副本不能保证擦除。
Future<BootstrapResult> callBootstrap(
  BridgeCandidate bridge,
  String directory,
  String run,
  Map<String, Object?> action,
  Uint8List input, {
  String? pauseAt,
  int abi = 1,
}) async {
  final metadata = jsonEncode({
    'abi': abi,
    'directory': directory,
    'run': run,
    'action': action,
    'pause_at': pauseAt,
  });
  final transfer = TransferableTypedData.fromList([input]);
  input.fillRange(0, input.length, 0);
  return Isolate.run(() async {
    final secret = transfer.materialize().asUint8List();
    try {
      String status;
      Uint8List output;
      if (bridge == BridgeCandidate.thin) {
        (status, output) = runThinBootstrap(metadata, secret);
      } else {
        await RustLib.init(
          externalLibrary: ExternalLibrary.open(loadedLibraryPath()),
        );
        try {
          final result = await frb.runBootstrap(
            requestJson: metadata,
            secret: secret,
          );
          status = result.statusJson;
          output = result.secret;
        } finally {
          RustLib.dispose();
        }
      }
      try {
        return BootstrapResult(
          jsonDecode(status) as Map<String, dynamic>,
          output,
        );
      } catch (_) {
        output.fillRange(0, output.length, 0);
        rethrow;
      }
    } finally {
      secret.fillRange(0, secret.length, 0);
    }
  });
}

/// 平台只处理本次 run / generation 的系统项，账务与封装仍由 Rust 决定。
class SystemProtection {
  static const channel = MethodChannel('radishledger/m0c/protection');
  Future<T?> _call<T>(String method, [Map<String, Object?>? arguments]) async {
    try {
      return await channel.invokeMethod<T>(method, arguments);
    } on PlatformException catch (error) {
      throw BootstrapFailure(
        error.code,
        error.details is Map ? (error.details as Map)['platformCode'] : null,
      );
    }
  }

  Future<String> storage() async {
    final path = await _call<String>('storage');
    if (path == null || path.isEmpty) {
      throw const BootstrapFailure('storage_unavailable');
    }
    return path;
  }

  Future<Map<String, dynamic>> capabilities() async {
    final result = await _call<Map>('capabilities');
    if (result == null || result['supported'] is! bool) {
      throw const BootstrapFailure('invalid_response');
    }
    return Map<String, dynamic>.from(result);
  }

  Future<Map<String, dynamic>> create(
    String run,
    String generation,
    Uint8List secret,
  ) async {
    try {
      final result = await _call<Map>('create', {
        'run': run,
        'generation': generation,
        'secret': secret,
      });
      if (result == null || result['created'] != true) {
        throw const BootstrapFailure('invalid_response');
      }
      return Map<String, dynamic>.from(result);
    } finally {
      secret.fillRange(0, secret.length, 0);
    }
  }

  Future<Uint8List> read(String run, String generation) async {
    final secret = await _call<Uint8List>('read', {
      'run': run,
      'generation': generation,
    });
    if (secret == null || secret.length != 32) {
      throw const BootstrapFailure('corrupt_protection');
    }
    // StandardMessageCodec 可能返回只读 typed data；只清理自己持有的可写副本。
    // 平台 codec 的原始副本不承诺可擦除。
    return Uint8List.fromList(secret);
  }

  Future<Map<String, dynamic>> delete(String run, String generation) async {
    final result = await _call<Map>('delete', {
      'run': run,
      'generation': generation,
    });
    if (result == null || result['deleted'] != true) {
      throw const BootstrapFailure('invalid_response');
    }
    return Map<String, dynamic>.from(result);
  }

  Future<String> control({bool remove = false}) async {
    final root = await _call<String>(remove ? 'removeControl' : 'control');
    if (root == null || root.isEmpty) {
      throw const BootstrapFailure('control_unavailable');
    }
    return root;
  }

  Future<void> cancel() async {
    await _call<void>('cancel');
  }
}

class BootstrapService {
  BootstrapService(this.directory, this.run, this.bridge, this.platform);
  final String directory;
  final String run;
  final BridgeCandidate bridge;
  final SystemProtection platform;
  int _epoch = 0;
  Future<void> lock() async {
    _epoch += 1;
    await platform.cancel();
  }

  Future<BootstrapResult> core(
    Map<String, Object?> action,
    Uint8List input, {
    String? pauseAt,
  }) => callBootstrap(bridge, directory, run, action, input, pauseAt: pauseAt);

  Future<BootstrapResult> unlockSystem() async {
    final epoch = _epoch;
    final status = await core({'kind': 'status'}, Uint8List(0));
    status.requireSuccess();
    final generation = status.report['activeProtection'] as String?;
    status.clear();
    if (generation == null) throw const BootstrapFailure('protection_missing');
    final secret = await platform.read(run, generation);
    if (epoch != _epoch) {
      secret.fillRange(0, secret.length, 0);
      throw const BootstrapFailure('interrupted');
    }
    final result = await core({'kind': 'inspect', 'unlock': 'system'}, secret);
    if (epoch != _epoch) {
      result.clear();
      throw const BootstrapFailure('interrupted');
    }
    result.requireSuccess();
    return result;
  }

  Future<BootstrapResult> establishProtection(
    String unlock,
    Uint8List input,
  ) async {
    try {
      final epoch = _epoch;
      final capability = await platform.capabilities();
      if (capability['supported'] != true) {
        input.fillRange(0, input.length, 0);
        throw BootstrapFailure('unsupported', capability['platformCode']);
      }
      if (epoch != _epoch) {
        input.fillRange(0, input.length, 0);
        throw const BootstrapFailure('interrupted');
      }
      final prepared = await core({
        'kind': 'prepareProtection',
        'unlock': unlock,
      }, input);
      prepared.requireSuccess();
      final generation = prepared.report['candidateProtection'] as String;
      try {
        if (epoch != _epoch) throw const BootstrapFailure('interrupted');
        await platform.create(
          run,
          generation,
          Uint8List.fromList(prepared.secret),
        );
        if (epoch != _epoch) throw const BootstrapFailure('interrupted');
        final verified = await platform.read(run, generation);
        if (epoch != _epoch) {
          verified.fillRange(0, verified.length, 0);
          throw const BootstrapFailure('interrupted');
        }
        final committed = await core({
          'kind': 'commitProtection',
          'generation': generation,
        }, verified);
        committed.requireSuccess();
        if (epoch != _epoch) {
          committed.clear();
          throw const BootstrapFailure('interrupted');
        }
        final previous = committed.report['previousProtection'] as String?;
        if (previous != null) await platform.delete(run, previous);
        return committed;
      } finally {
        prepared.clear();
      }
    } finally {
      input.fillRange(0, input.length, 0);
    }
  }
}
