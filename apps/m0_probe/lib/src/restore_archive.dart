import 'dart:io';
import 'package:flutter/services.dart';
import 'bootstrap.dart';
import 'internal_ledger.dart';
import 'probe.dart';

const archivePicker = MethodChannel('radishledger/internal/archive-picker');
const archiveInput = MethodChannel('radishledger/internal/archive-input');

/// 输入为系统选择后复制的候选文件。独立暂存钥只在本次核对期间持有。
class RestoreArchive {
  RestoreArchive(this.directory, this.stage);
  final String directory;
  final String stage;
  BootstrapResult? _prepared;
  bool get hasTarget => _prepared != null;

  static Future<RestoreArchive> copySelection(InternalLedger ledger) async {
    final limits = await ledger.service.core({
      'kind': 'restoreInputLimits',
    }, Uint8List(0));
    try {
      limits.requireSuccess();
      final max = limits.report['maxBytes'];
      final magic = limits.report['magic'];
      if (max is! int || max <= 0 || magic is! List || magic.isEmpty) {
        throw const BootstrapFailure('invalid_response');
      }
      final copy = await archiveInput.invokeMapMethod<String, dynamic>('copy', {
        'limit': max,
        'magic': Uint8List.fromList(List<int>.from(magic)),
      });
      final directory = copy?['directory'];
      final stage = copy?['stage'];
      if (directory is! String ||
          !directory.startsWith('/') ||
          stage is! String ||
          !RegExp(r'^archive-[0-9a-f]{32}$').hasMatch(stage)) {
        throw const BootstrapFailure('invalid_response');
      }
      return RestoreArchive(directory, stage);
    } finally {
      limits.clear();
    }
  }

  Future<BootstrapResult> _call(Map<String, Object?> action, Uint8List input) =>
      callBootstrap(BridgeCandidate.thin, directory, InternalLedger.run, {
        ...action,
        'stage': stage,
      }, input);

  Future<Map<String, dynamic>> review(Uint8List password) async {
    final prepared = _prepared;
    BootstrapResult? result;
    try {
      if (password.isEmpty || password.length > 1024) {
        throw const BootstrapFailure('invalid_password');
      }
      if (prepared == null) {
        result = await _call({'kind': 'prepareRestore'}, password);
        result.requireSuccess();
        if (result.secret.length != 32 || result.report['ticket'] is! Map) {
          throw const BootstrapFailure('invalid_response');
        }
        _prepared = result;
      } else {
        final input = Uint8List(32 + password.length)
          ..setAll(0, prepared.secret)
          ..setAll(32, password);
        result = await _call({
          'kind': 'inspectRestore',
          'ticket': prepared.report['ticket'],
        }, input);
        result.requireSuccess();
      }
      final report = result.report;
      for (final field in [
        'accounts',
        'categories',
        'activeEvents',
        'revisions',
        'intents',
        'deleted',
        'candidates',
        'sequence',
      ]) {
        if (report[field] is! int || (report[field] as int) < 0) {
          throw const BootstrapFailure('invalid_response');
        }
      }
      for (final field in [
        'netAssetsMinor',
        'assetsMinor',
        'liabilitiesMinor',
      ]) {
        if (report[field] is! String ||
            !RegExp(r'^-?(0|[1-9][0-9]*)$').hasMatch(report[field])) {
          throw const BootstrapFailure('invalid_response');
        }
      }
      final balances = report['balances'];
      if (balances is! List ||
          balances.length != (report['accounts'] as int).clamp(0, 8) ||
          balances.any(
            (row) =>
                row is! Map ||
                row['name'] is! String ||
                row['minor'] is! String ||
                !RegExp(
                  r'^-?(0|[1-9][0-9]*)$',
                ).hasMatch(row['minor'] as String),
          )) {
        throw const BootstrapFailure('invalid_response');
      }
      return report;
    } finally {
      password.fillRange(0, password.length, 0);
      if (!identical(result, _prepared)) result?.clear();
    }
  }

  Future<void> discard() async {
    final prepared = _prepared;
    if (prepared != null) {
      final result = await _call({
        'kind': 'discardRestore',
        'ticket': prepared.report['ticket'],
      }, Uint8List.fromList(prepared.secret));
      try {
        result.requireSuccess();
        if (result.report['discarded'] != true) {
          throw const BootstrapFailure('invalid_response');
        }
      } finally {
        result.clear();
      }
      prepared.clear();
      _prepared = null;
    }
    final dir = Directory('$directory/$stage');
    final file = File('${dir.path}/source.rla');
    // 只有输入副本及空父目录由界面清理，SQLCipher 目标的删除由 Rust 核对归属。
    if (!await dir.exists()) return;
    await for (final member in dir.list(followLinks: false)) {
      if (member is! File || member.path != file.path) {
        throw const BootstrapFailure('restore_mismatch');
      }
    }
    if (await file.exists()) await file.delete();
    await dir.delete();
  }

  void forgetKey() => _prepared?.clear();
}
