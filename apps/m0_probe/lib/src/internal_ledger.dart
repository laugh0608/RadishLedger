import 'dart:typed_data';

import 'bootstrap.dart';
import 'bootstrap_selection.dart';
import 'probe.dart';

enum InternalLedgerState { empty, initializing, registered }

enum LedgerUnlock { password, recovery }

typedef LedgerIdentity = (String, String);
typedef InternalLedgerLocation = ({
  InternalLedgerState state,
  LedgerIdentity? identity,
});

/// Android 实验目录内的单一位置。账簿与设备身份由 Rust 独立生成。
class InternalLedger {
  InternalLedger(String directory, SystemProtection platform)
    : service = BootstrapService(
        directory,
        run,
        BridgeCandidate.thin,
        platform,
      );

  static const run = '00000000000000000000000000000001';
  final BootstrapService service;

  static Future<InternalLedger> open(SystemProtection platform) async =>
      InternalLedger(await platform.internalLedgerStorage(), platform);

  static LedgerIdentity identity(Map<String, dynamic> report) {
    final vault = report['vault'];
    final device = report['device'];
    if (!BootstrapSelection.validIdentity(vault) ||
        !BootstrapSelection.validIdentity(device)) {
      throw const BootstrapFailure('invalid_response');
    }
    return (vault as String, device as String);
  }

  Future<InternalLedgerLocation> locate() async {
    final result = await service.core({'kind': 'locate'}, Uint8List(0));
    try {
      result.requireSuccess();
      return switch (result.report['state']) {
        'empty' => (state: InternalLedgerState.empty, identity: null),
        'initializing' => (
          state: InternalLedgerState.initializing,
          identity: identity(result.report),
        ),
        'registered' => (
          state: InternalLedgerState.registered,
          identity: identity(result.report),
        ),
        _ => throw const BootstrapFailure('invalid_response'),
      };
    } finally {
      result.clear();
    }
  }

  Future<BootstrapResult> create(
    Uint8List password, {
    bool includeLedger = false,
  }) async {
    BootstrapResult? generated;
    try {
      if (password.isEmpty || password.length > 1024) {
        throw const BootstrapFailure('invalid_password');
      }
      generated = await service.core({'kind': 'newRecovery'}, Uint8List(0));
      generated.requireSuccess();
      if (generated.secret.length != 80) {
        throw const BootstrapFailure('invalid_response');
      }
      final input = Uint8List(password.length + generated.secret.length)
        ..setAll(0, password)
        ..setAll(password.length, generated.secret);
      final result = await _authenticated(
        {'kind': 'create'},
        input,
        null,
        includeLedger: includeLedger,
      );
      try {
        return BootstrapResult(
          result.status,
          Uint8List.fromList(generated.secret),
        );
      } finally {
        result.clear();
      }
    } finally {
      password.fillRange(0, password.length, 0);
      generated?.clear();
    }
  }

  Future<BootstrapResult> unlock(
    InternalLedgerLocation location,
    LedgerUnlock unlock,
    Uint8List input, {
    bool includeLedger = false,
  }) => _authenticated(
    {
      'kind': location.state == InternalLedgerState.initializing
          ? 'resume'
          : 'inspect',
      'unlock': unlock.name,
    },
    input,
    location.identity,
    includeLedger: includeLedger,
  );

  Future<BootstrapResult> confirm(
    LedgerIdentity expected,
    Uint8List input, {
    bool includeLedger = false,
  }) => _authenticated(
    {'kind': 'confirm'},
    input,
    expected,
    includeLedger: includeLedger,
  );

  Future<BootstrapResult> reissue(LedgerIdentity expected, Uint8List input) =>
      _authenticated({'kind': 'reissueRecovery'}, input, expected);

  Future<BootstrapResult> save(
    LedgerIdentity expected,
    LedgerUnlock unlock,
    Uint8List input,
    Map<String, Object?> command,
  ) => _authenticated(
    {'kind': 'saveLocal', 'unlock': unlock.name, 'command': command},
    input,
    expected,
  );

  Future<BootstrapResult> _authenticated(
    Map<String, Object?> action,
    Uint8List input,
    LedgerIdentity? expected, {
    bool includeLedger = false,
  }) async {
    final result = await callBootstrap(
      BridgeCandidate.thin,
      service.directory,
      run,
      action,
      input,
      expectedIdentity: expected,
      includeLedger: includeLedger,
    );
    try {
      result.requireSuccess();
      final actual = identity(result.report);
      if (expected != null && expected != actual) {
        throw const BootstrapFailure('selection_changed');
      }
      if (result.report['ready'] is! bool) {
        throw const BootstrapFailure('invalid_response');
      }
      return result;
    } catch (_) {
      result.clear();
      rethrow;
    }
  }
}
