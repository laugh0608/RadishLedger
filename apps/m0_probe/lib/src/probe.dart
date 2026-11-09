import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'frb_generated/api.dart' as frb;
import 'frb_generated/frb_generated.dart';
import 'native.dart';

enum BridgeCandidate { thin, frb }

class ProbeReport {
  ProbeReport(this.data) {
    if (data['fixtureVersion'] != 1 ||
        data['caseCount'] is! int ||
        data['checkpointCount'] is! int ||
        data['cipherVersion'] is! String ||
        data['nativeTarget'] is! String ||
        data['cases'] is! List<Object?> ||
        data['duplicateVerified'] != true ||
        data['rollbackVerified'] != true ||
        data['reopenVerified'] != true ||
        data['wrongKeyRejected'] != true) {
      throw const FormatException('核心返回的验证结果格式不兼容或缺少必要证据');
    }
    if (cipherVersion.isEmpty ||
        nativeTarget.isEmpty ||
        caseCount <= 0 ||
        checkpointCount < caseCount ||
        (data['cases']! as List<Object?>).length != caseCount) {
      throw const FormatException('核心没有实际完成有效案例');
    }
  }
  final Map<String, Object?> data;
  int get caseCount => data['caseCount']! as int;
  int get checkpointCount => data['checkpointCount']! as int;
  String get cipherVersion => data['cipherVersion']! as String;
  String get nativeTarget => data['nativeTarget']! as String;
}

Future<ProbeReport> runProbe(BridgeCandidate candidate) async {
  final response = await Isolate.run(() async {
    if (candidate == BridgeCandidate.thin) {
      return runThin(Directory.systemTemp.path);
    }
    await RustLib.init(
      externalLibrary: ExternalLibrary.open(loadedLibraryPath()),
    );
    try {
      return await frb.runProbe(directory: Directory.systemTemp.path);
    } finally {
      RustLib.dispose();
    }
  });
  final decoded = jsonDecode(response);
  if (decoded is! Map<String, Object?> || decoded['ok'] is! bool) {
    throw const FormatException('核心返回了不兼容的结果');
  }
  if (decoded['ok'] != true) {
    final error = decoded['error'];
    if (error is! Map<String, Object?> ||
        error['code'] is! String ||
        error['message'] is! String) {
      throw const FormatException('核心错误格式不兼容');
    }
    throw StateError('${error['code']}: ${error['message']}');
  }
  final report = decoded['report'];
  if (report is! Map<String, Object?>) throw const FormatException('核心结果缺少报告');
  return ProbeReport(report);
}
