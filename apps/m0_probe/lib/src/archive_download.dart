import 'dart:io';
import 'package:flutter/services.dart';
import 'bootstrap.dart';
import 'probe.dart';

const _downloads = MethodChannel('radishledger/internal/archive');

/// 只有本次新建的下载项能够发布。离开前台时在发布点前停止。
Future<BootstrapResult> exportArchive({
  required String directory,
  required String run,
  required (String, String) identity,
  required String unlock,
  required String name,
  required Uint8List local,
  required Uint8List password,
  required bool Function() active,
}) async {
  Directory? temporary;
  BootstrapResult? prepared;
  var discard = false;
  var publishing = false;
  void checkActive() {
    if (!active()) throw const BootstrapFailure('cancelled');
  }

  try {
    if (password.isEmpty ||
        password.length > 80 ||
        local.isEmpty ||
        local.length > 1024) {
      throw const BootstrapFailure('invalid_archive_password');
    }
    temporary = await Directory(directory).createTemp('archive-');
    final stage = temporary.uri.pathSegments.where((s) => s.isNotEmpty).last;
    checkActive();
    final secret = Uint8List(local.length + password.length)
      ..setAll(0, local)
      ..setAll(local.length, password);
    prepared = await callBootstrap(
      BridgeCandidate.thin,
      directory,
      run,
      {
        'kind': 'prepareArchive',
        'unlock': unlock,
        'stage': stage,
        'archive_bytes': password.length,
      },
      secret,
      expectedIdentity: identity,
    );
    prepared.requireSuccess();
    final digest = prepared.report['digest'];
    if (prepared.report['vault'] != identity.$1 ||
        prepared.report['device'] != identity.$2 ||
        prepared.report['ready'] != true ||
        prepared.secret.isNotEmpty ||
        digest is! List ||
        digest.length != 32 ||
        !digest.every((n) => n is int && n >= 0 && n <= 255)) {
      throw const BootstrapFailure('invalid_response');
    }
    checkActive();
    discard = true;
    await _downloads.invokeMethod<void>('prepare', {
      'stage': stage,
      'name': '$name.rla',
    });
    checkActive();
    final verified = await callBootstrap(BridgeCandidate.thin, directory, run, {
      'kind': 'verifyArchive',
      'stage': stage,
      'digest': digest,
    }, Uint8List.fromList(password));
    try {
      verified.requireSuccess();
      if (verified.report['verified'] != true || verified.secret.isNotEmpty) {
        throw const BootstrapFailure('invalid_response');
      }
    } finally {
      verified.clear();
    }
    checkActive();
    publishing = true;
    discard = false;
    final actual = await _downloads.invokeMethod<String>('publish', {
      'stage': stage,
    });
    if (actual == null || actual.isEmpty) {
      throw const BootstrapFailure('archive_publish_uncertain');
    }
    prepared.report['archiveName'] = actual;
    return prepared;
  } on PlatformException catch (error) {
    throw BootstrapFailure(
      publishing ? 'archive_publish_uncertain' : error.code,
    );
  } finally {
    local.fillRange(0, local.length, 0);
    password.fillRange(0, password.length, 0);
    prepared?.clear();
    if (temporary != null) {
      try {
        if (discard) {
          await _downloads.invokeMethod<void>('discard', {
            'stage': temporary.uri.pathSegments.where((s) => s.isNotEmpty).last,
          });
        }
      } on PlatformException {
        throw const BootstrapFailure('archive_cleanup_failed');
      } finally {
        try {
          await temporary.delete(recursive: true);
        } on FileSystemException {
          throw BootstrapFailure(
            publishing ? 'archive_publish_uncertain' : 'archive_cleanup_failed',
          );
        }
      }
    }
  }
}
