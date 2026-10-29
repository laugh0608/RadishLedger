// 同一合成账库的进程内基线；不保存解锁能力，不替代真机保护验收。
import 'dart:math';
import 'package:flutter/foundation.dart';
import 'bootstrap.dart';
import 'bootstrap_selection.dart';

enum LockFileOutcome {
  notStarted,
  observing,
  observed,
  cancelled,
  lockedIoNotExecuted,
  restorationNotObserved,
  baselineUnavailable,
  baselineFailed,
  unsupportedSimulator,
  unsupportedPlatform,
  failed,
}

enum LockReviewState {
  baselineSaved,
  awaitingObservation,
  awaitingUnlock,
  matched,
  mismatch,
  unlockFailed,
  interrupted,
}

final lockFileOutcomes = Set<String>.unmodifiable(
  LockFileOutcome.values.map((value) => value.name),
);
final lockReviewStates = Set<String>.unmodifiable(
  LockReviewState.values.map((value) => value.name),
);
const lockFileCoverage = {'observed', 'absentAtStart', 'notObserved'};

/// 完整事实由 Rust contentDigest 覆盖；这里只保存身份、摘要及独立 A11 计数。
Map<String, Object> _baseline(Map<String, dynamic> report) {
  final audit = report['audit'];
  final snapshot = audit is Map ? audit['snapshot'] : null;
  if (report['ready'] != true ||
      report['fixtureComplete'] != true ||
      !BootstrapSelection.validIdentity(report['vault']) ||
      !BootstrapSelection.validIdentity(report['device']) ||
      report['contentDigest'] is! String ||
      !RegExp(r'^[0-9a-f]{64}$').hasMatch(report['contentDigest'] as String) ||
      audit is! Map ||
      snapshot is! Map ||
      audit['sequence'] != 8 ||
      audit['revisions'] != 8 ||
      audit['operations'] is! List ||
      (audit['operations'] as List).length != 7 ||
      snapshot['deletedEventIds'] is! List ||
      (snapshot['deletedEventIds'] as List).length != 1 ||
      snapshot['unresolvedCandidateRevisionIds'] is! List ||
      (snapshot['unresolvedCandidateRevisionIds'] as List).length != 2 ||
      snapshot['netAssetsMinor'] != '18000') {
    throw const BootstrapFailure('lock_baseline_invalid');
  }
  return Map.unmodifiable({
    'vault': report['vault'] as String,
    'device': report['device'] as String,
    'contentDigest': report['contentDigest'] as String,
    'sequence': 8,
    'revisions': 8,
    'operationCount': 7,
    'candidateCount': 2,
    'deletedCount': 1,
  });
}

Future<BootstrapResult> _inspect(
  BootstrapService service,
  String unlock,
  Uint8List input,
) async {
  try {
    if (unlock == 'system') return await service.unlockSystem();
    if (unlock != 'password' && unlock != 'recovery') {
      throw const BootstrapFailure('invalid_argument');
    }
    final result = await service.core({
      'kind': 'inspect',
      'unlock': unlock,
    }, input);
    result.requireSuccess();
    return result;
  } finally {
    input.fillRange(0, input.length, 0);
  }
}

class BootstrapLockReview {
  BootstrapLockReview._(this.service, this.baseline)
    : id = List.generate(
        16,
        (_) => Random.secure().nextInt(256).toRadixString(16).padLeft(2, '0'),
      ).join();
  final BootstrapService service;
  final String id;
  final Map<String, Object> baseline;
  LockReviewState state = LockReviewState.baselineSaved;
  LockFileOutcome fileOutcome = LockFileOutcome.notStarted;
  final Map<String, Object> _files = {};
  String? _afterDigest;
  bool get observing => fileOutcome == LockFileOutcome.observing;
  bool get canVerify => fileOutcome != LockFileOutcome.notStarted && !observing;

  static Future<BootstrapLockReview> prepare(
    BootstrapService service,
    String unlock,
    Uint8List input,
  ) async {
    final result = await _inspect(service, unlock, input);
    try {
      return BootstrapLockReview._(service, _baseline(result.report));
    } finally {
      result.clear();
    }
  }

  Map<String, dynamic> get report => {
    'lockReview': {
      'reviewId': id,
      'reviewState': state.name,
      'fileOutcome': fileOutcome.name,
      'baselineDigest': baseline['contentDigest'],
      'afterDigest': ?_afterDigest,
      ...baseline,
      ..._files,
    },
  };

  Future<BootstrapResult> observe() async {
    if (fileOutcome != LockFileOutcome.notStarted) {
      throw const BootstrapFailure('lock_observation_already_started');
    }
    state = LockReviewState.awaitingObservation;
    fileOutcome = LockFileOutcome.observing;
    try {
      final value = await service.platform.observeBootstrapFiles(
        service.run,
        id,
      );
      final outcome = value['outcome'];
      if (value['run'] != service.run ||
          value['observation'] != id ||
          value['schema'] != 1 ||
          !lockFileOutcomes.contains(outcome) ||
          ['notStarted', 'observing', 'failed'].contains(outcome) ||
          value['hardwareProtectionVerified'] != false ||
          value['acceptance'] != 'not-assessed') {
        throw const BootstrapFailure('invalid_response');
      }
      fileOutcome = LockFileOutcome.values.byName(outcome as String);
      for (final key in [
        'lockedAttempted',
        'accessDeniedObserved',
        'accessRestored',
        'coveredFilesDenied',
        'coveredFilesRestored',
        'controlWriteDeniedObserved',
        'controlWriteRestored',
      ]) {
        if (value[key] is bool) _files[key] = value[key] as bool;
      }
      final coverage = value['coverage'];
      for (final (name, key) in [
        ('ledger.db', 'dbCoverage'),
        ('ledger.db-wal', 'walCoverage'),
        ('ledger.db-shm', 'shmCoverage'),
      ]) {
        if (coverage is! Map || !lockFileCoverage.contains(coverage[name])) {
          throw const BootstrapFailure('invalid_response');
        }
        _files[key] = coverage[name] as String;
      }
      if (outcome != 'unsupportedSimulator' &&
          outcome != 'unsupportedPlatform') {
        final resources = value['resources'];
        _files['resourcesClosed'] =
            resources is Map &&
            resources['writeControlCloseErrno'] == 0 &&
            resources['directoryCloseErrno'] == 0;
        if (_files['resourcesClosed'] != true) {
          throw const BootstrapFailure('lock_resources_unconfirmed');
        }
      }
      state = LockReviewState.awaitingUnlock;
      return BootstrapResult({'ok': true, 'report': report}, Uint8List(0));
    } catch (_) {
      fileOutcome = LockFileOutcome.failed;
      rethrow;
    } finally {
      state = LockReviewState.awaitingUnlock;
    }
  }

  Future<void> cancelObservation() async {
    if (observing) {
      await service.platform.cancelBootstrapObservation(service.run, id);
    }
  }

  Future<BootstrapResult> verify(
    BootstrapService selected,
    String unlock,
    Uint8List input, {
    required bool Function() isCurrent,
  }) async {
    BootstrapResult? result;
    try {
      if (!identical(selected, service)) {
        throw const BootstrapFailure('selection_changed');
      }
      if (!canVerify) {
        throw const BootstrapFailure('lock_observation_incomplete');
      }
      _afterDigest = null;
      result = await _inspect(service, unlock, input);
      if (!isCurrent()) throw const BootstrapFailure('interrupted');
      Map<String, Object> after;
      try {
        after = _baseline(result.report);
      } on BootstrapFailure {
        throw const BootstrapFailure('lock_review_mismatch');
      }
      _afterDigest = after['contentDigest'] as String;
      if (!mapEquals(baseline, after)) {
        throw const BootstrapFailure('lock_review_mismatch');
      }
      state = LockReviewState.matched;
      return BootstrapResult({'ok': true, 'report': report}, Uint8List(0));
    } on BootstrapFailure catch (error) {
      state = error.code == 'interrupted'
          ? LockReviewState.interrupted
          : ['lock_review_mismatch', 'fixture_mismatch'].contains(error.code)
          ? LockReviewState.mismatch
          : LockReviewState.unlockFailed;
      rethrow;
    } catch (_) {
      state = LockReviewState.unlockFailed;
      rethrow;
    } finally {
      input.fillRange(0, input.length, 0);
      result?.clear();
    }
  }
}
