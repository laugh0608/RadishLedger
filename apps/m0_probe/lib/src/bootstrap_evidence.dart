// 只投影合成实验的非秘密观测，不接受口令 / 恢复材料，不自动写文件或日志。
import 'dart:convert';
import 'dart:io';
import 'dart:math';
import 'package:flutter/foundation.dart';
import 'bootstrap.dart';
import 'bootstrap_lock_review.dart';
import 'probe.dart';
import 'probe_report.dart';

enum BootstrapAction {
  create,
  confirmRecovery,
  reissueRecovery,
  seed,
  resume,
  unlockPassword,
  unlockRecovery,
  unlockSystem,
  establishPassword,
  establishRecovery,
  removeProtection,
  cleanup,
  lock,
  lockBaseline,
  lockObserve,
  lockVerify,
}

bool _hex(Object? value, int length) =>
    value is String && RegExp('^[0-9a-f]{$length}\$').hasMatch(value);

Object? _platformCode(Object? value) {
  if (value is int && value.abs() <= 2147483647) return value;
  if (value is String && RegExp(r'^-?[0-9]{1,10}$').hasMatch(value)) {
    final number = int.tryParse(value);
    if (number != null && number.abs() <= 2147483647) return number;
  }
  const classes = {
    'KeyPermanentlyInvalidatedException',
    'UserNotAuthenticatedException',
    'AEADBadTagException',
    'IOException',
    'FileNotFoundException',
    'KeyStoreException',
    'InvalidKeyException',
    'ProviderException',
    'UnrecoverableKeyException',
    'IllegalStateException',
    'SecurityException',
  };
  return classes.contains(value) ? value : null;
}

const _errors = {
  'abi_mismatch',
  'invalid_argument',
  'invalid_response',
  'invalid_password',
  'invalid_recovery',
  'authentication_failed',
  'unsupported',
  'item_missing',
  'item_invalidated',
  'item_exists',
  'protection_missing',
  'material_missing',
  'user_cancelled',
  'authentication_denied',
  'authentication_required',
  'temporarily_unavailable',
  'authentication_unavailable',
  'interrupted',
  'already_exists',
  'recovery_unconfirmed',
  'already_ready',
  'not_ready',
  'missing_database',
  'invalid_bootstrap',
  'corrupt_protection',
  'storage_error',
  'storage_unavailable',
  'bootstrap_io',
  'bootstrap_publish_uncertain',
  'foreign_directory',
  'invalid_directory',
  'invalid_file',
  'fixture_mismatch',
  'kdf_busy',
  'vault_busy',
  'protection_busy',
  'selection_changed',
  'invalid_selection',
  'selection_publish_incomplete',
  'cleanup_failed',
  'verification_failed',
  'platform_error',
  'file_boundary_unavailable',
  'lock_baseline_invalid',
  'lock_review_mismatch',
  'lock_resources_unconfirmed',
  'lock_observation_incomplete',
  'lock_observation_already_started',
};

Map<String, Object> _coreSummary(Map<String, dynamic> report) {
  final result = <String, Object>{};
  for (final field in [
    'vault',
    'device',
    'activeProtection',
    'protectionRemoved',
  ]) {
    if (_hex(report[field], 32)) result[field] = report[field] as String;
  }
  if (_hex(report['contentDigest'], 64)) {
    result['contentDigest'] = report['contentDigest'] as String;
  }
  for (final field in [
    'ready',
    'fixtureComplete',
    'cleaned',
    'alreadyMissing',
  ]) {
    if (report[field] is bool) result[field] = report[field] as bool;
  }
  for (final field in ['pid', 'removedProtectionCount']) {
    final value = report[field];
    if (value is int && value >= 0 && value <= 2147483647) {
      result[field] = value;
    }
  }
  final audit = report['audit'];
  if (audit is Map) {
    for (final field in ['sequence', 'revisions']) {
      final value = audit[field];
      if (value is int && value >= 0 && value <= 2147483647) {
        result[field] = value;
      }
    }
    if (audit['operations'] is List) {
      result['operationCount'] = (audit['operations'] as List).length;
    }
    final snapshot = audit['snapshot'];
    if (snapshot is Map) {
      for (final (source, target) in [
        ('deletedEventIds', 'deletedCount'),
        ('unresolvedCandidateRevisionIds', 'candidateCount'),
      ]) {
        if (snapshot[source] is List) {
          result[target] = (snapshot[source] as List).length;
        }
      }
    }
  }
  final protection = report['platformProtection'];
  if (protection is Map &&
      {
        '-2',
        '-1',
        '0',
        '1',
        '2',
        'unmeasured',
      }.contains(protection['securityLevelCode'])) {
    result['securityLevelCode'] = protection['securityLevelCode'] as String;
  }
  final review = report['lockReview'];
  if (review is Map) {
    final projected = <String, Object>{};
    for (final key in ['reviewId', 'vault', 'device']) {
      if (_hex(review[key], 32)) projected[key] = review[key] as String;
    }
    for (final key in ['baselineDigest', 'afterDigest']) {
      if (_hex(review[key], 64)) projected[key] = review[key] as String;
    }
    for (final (key, allowed) in [
      ('reviewState', lockReviewStates),
      ('fileOutcome', lockFileOutcomes),
      ('dbCoverage', lockFileCoverage),
      ('walCoverage', lockFileCoverage),
      ('shmCoverage', lockFileCoverage),
    ]) {
      if (allowed.contains(review[key])) projected[key] = review[key] as String;
    }
    for (final key in [
      'lockedAttempted',
      'accessDeniedObserved',
      'accessRestored',
      'coveredFilesDenied',
      'coveredFilesRestored',
      'controlWriteDeniedObserved',
      'controlWriteRestored',
      'resourcesClosed',
    ]) {
      if (review[key] is bool) projected[key] = review[key] as bool;
    }
    for (final key in [
      'sequence',
      'revisions',
      'operationCount',
      'candidateCount',
      'deletedCount',
    ]) {
      final value = review[key];
      if (value is int && value >= 0 && value <= 2147483647) {
        projected[key] = value;
      }
    }
    result['lockReview'] = projected;
  }
  return result;
}

class BootstrapObservation {
  BootstrapObservation._(this.sequence, this.action, this.run, this.bridge);
  final int sequence;
  final BootstrapAction action;
  final String run;
  final BridgeCandidate bridge;
  final _clock = Stopwatch()..start();
  bool _waitStopped = false;
  String _outcome = 'inProgress';
  String? _error;
  Object? _platform;
  Map<String, Object> _summary = {};

  void stopWaiting() => _waitStopped = true;
  void success(Map<String, dynamic> report) {
    final summary = _coreSummary(report);
    _finish(
      report['actionCancelled'] == true ? 'cancelledBeforeAction' : 'succeeded',
    );
    _summary = summary;
  }

  void failure(BootstrapFailure error, {Map<String, dynamic>? report}) {
    if (report != null) _summary = _coreSummary(report);
    _finish('failed');
    _error = _errors.contains(error.code) ? error.code : 'unrecognized_error';
    _platform = _platformCode(error.platformCode);
  }

  void unknown() => _finish('unknown');
  void _finish(String outcome) {
    if (_outcome != 'inProgress') throw StateError('操作结果已记录');
    _outcome = outcome;
    _clock.stop();
  }

  Map<String, Object> _json() => {
    'sequence': sequence,
    'action': action.name,
    'run': run,
    'bridge': bridge.name,
    'outcome': _outcome,
    'waitStopped': _waitStopped,
    'elapsedMicros': _clock.elapsedMicroseconds,
    'errorCode': ?_error,
    'platformCode': ?_platform,
    'summary': Map<String, Object>.from(_summary),
  };
}

class BootstrapEvidence {
  BootstrapEvidence({
    this.session = const String.fromEnvironment('M0C_EVIDENCE_SESSION'),
  });
  final String session;
  static const maxObservations = 64;
  final _observations = <BootstrapObservation>[];
  final _capabilities = <String, Object>{};
  int _sequence = 0;
  int _omitted = 0;
  bool get canPublish => _hex(session, 32);

  void capabilities(Map<String, dynamic> value) {
    _capabilities.clear();
    for (final field in ['supported', 'deviceSecure']) {
      if (value[field] is bool) _capabilities[field] = value[field] as bool;
    }
    if (value['api'] is int && value['api'] >= 0 && value['api'] <= 999) {
      _capabilities['api'] = value['api'] as int;
    }
    final code = _platformCode(value['platformCode']);
    if (code != null) _capabilities['platformCode'] = code;
    if (['simulator', 'device'].contains(value['environment'])) {
      _capabilities['environment'] = value['environment'] as String;
    }
  }

  BootstrapObservation begin(
    BootstrapAction action,
    String run,
    BridgeCandidate bridge,
  ) {
    if (!_hex(run, 32)) throw StateError('合成实验身份无效');
    if (_observations.length == maxObservations) {
      _observations.removeAt(0);
      _omitted++;
    }
    final observation = BootstrapObservation._(
      ++_sequence,
      action,
      run,
      bridge,
    );
    _observations.add(observation);
    return observation;
  }

  BootstrapEvidenceReport snapshot() {
    final random = Random.secure();
    final reportId = List.generate(
      16,
      (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
    ).join();
    return BootstrapEvidenceReport._({
      'schema': 1,
      'scope': 'synthetic-bootstrap-observations',
      'session': canPublish ? session : null,
      'reportId': reportId,
      'pid': pid,
      'platform': Platform.operatingSystem,
      'buildMode': kReleaseMode
          ? 'release'
          : kProfileMode
          ? 'profile'
          : 'debug',
      'hardwareVerified': false,
      'acceptance': 'not-assessed',
      'capabilities': Map<String, Object>.from(_capabilities),
      'omittedObservations': _omitted,
      'observations': _observations.map((value) => value._json()).toList(),
    });
  }
}

/// 构造器私有：输出只能来自上述固定字段投影，不能把任意 Map 当报告发送。
class BootstrapEvidenceReport {
  BootstrapEvidenceReport._(Map<String, dynamic> snapshot)
    : text = const JsonEncoder.withIndent('  ').convert(snapshot),
      _session = snapshot['session'] as String?,
      reportId = snapshot['reportId'] as String;
  final String text;
  final String? _session;
  final String reportId;
  bool get canPublish => _session != null;

  /// 仅由人工诊断弹窗显式调用；不建立持久记录或输出默认日志。
  Future<void> publish(void Function(String) output) async {
    if (!canPublish) {
      throw StateError('未配置有效合成诊断批次');
    }
    final chunks = probeReportChunks(utf8.encode(text));
    await emitProbeReport('RLM0_BOOTSTRAP:$_session:$reportId', chunks, output);
  }
}
