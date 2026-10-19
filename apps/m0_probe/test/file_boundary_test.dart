import 'dart:io';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/file_boundary.dart';
import 'package:radishledger_probe/src/native.dart';
import 'package:radishledger_probe/src/probe.dart';
import 'package:radishledger_probe/src/validation.dart';

Map<String, dynamic> android() => {
  'platform': 'android',
  'uid': 10200,
  'storage': '/private/no_backup/m0-c',
  'noBackup': '/private/no_backup',
  'debuggable': false,
  'allowBackup': false,
  'selinux': 'u:r:untrusted_app:s0:c100',
  'storageMetadata': {
    'path': '/private/no_backup/m0-c',
    'mode': 448,
    'uid': 10200,
  },
};

void main() {
  test(
    'missing Simulator protection stays unverified; devices and weaker classes reject',
    () {
      expect(
        fileProtectionEvidence(
          {'environment': 'simulator'},
          {'fileProtection': null},
        ),
        'unavailableOnSimulator',
      );
      expect(
        () => fileProtectionEvidence(
          {'environment': 'device'},
          {'fileProtection': null},
        ),
        throwsStateError,
      );
      expect(
        () => fileProtectionEvidence(
          {'environment': 'simulator'},
          {'fileProtection': 'NSFileProtectionNone'},
        ),
        throwsStateError,
      );
    },
  );
  test('release identity rejects run-as and debuggable processes', () {
    verifyAppEnvironment(android());
    expect(
      () => verifyAppEnvironment(android()..['selinux'] = 'u:r:runas_app:s0'),
      throwsStateError,
    );
    expect(
      () => verifyAppEnvironment(android()..['debuggable'] = true),
      throwsStateError,
    );
  });
  test('backup opt-in or foreign noBackup path cannot pass', () {
    expect(
      () => verifyAppEnvironment(android()..['allowBackup'] = true),
      throwsStateError,
    );
    expect(
      () => verifyAppEnvironment(android()..['noBackup'] = '/foreign'),
      throwsStateError,
    );
  });
  test(
    'a file with matching basename outside the owned directory is rejected',
    () {
      const run = '00000000000000000000000000000001';
      const root = '/private/no_backup/m0-c/m0c-$run';
      final attributes = <String, dynamic>{
        'root': {'path': root, 'mode': 448, 'uid': 10200},
        'files':
            [
                  'owner',
                  'lock',
                  'ledger.db',
                  'ledger.db-wal',
                  'ledger.db-shm',
                  'auxiliary.db',
                  'scanner-control.db',
                ]
                .map(
                  (name) => <String, dynamic>{
                    'path': '$root/$name',
                    'mode': 384,
                    'uid': 10200,
                    'selinux': {
                      'errno': 0,
                      'value': 'u:object_r:app_data_file:s0',
                    },
                  },
                )
                .toList(),
      };
      verifyAppFileAttributes(android(), attributes, run);
      (attributes['files'] as List).first['path'] = '/foreign/owner';
      expect(
        () => verifyAppFileAttributes(android(), attributes, run),
        throwsStateError,
      );
    },
  );
  for (final bridge in BridgeCandidate.values) {
    test(
      '${bridge.name} transfers named VFS evidence and exact cleanup',
      () async {
        final root = await Directory.systemTemp.createTemp(
          'radishledger-app-boundary-',
        );
        const run = '00000000000000000000000000000001';
        try {
          final result = await callValidation(bridge, {
            'kind': 'fileBoundary',
            'run': run,
            'fileControl': false,
          }, directory: root.path);
          expect(result['ok'], true);
          final report = result['report'] as Map;
          expect(report['defaultVfsUnchanged'], true);
          expect(report['fullFactsPreserved'], true);
          expect(report['a11Audit']['operations'], hasLength(7));
          expect(report['ioEventCount'], greaterThan(0));
          expect(report['walBytesForPlatformAudit'], greaterThan(32));
          expect(nativeBufferCount(), 0);
        } finally {
          if (await Directory('${root.path}/m0c-$run').exists()) {
            final result = await callValidation(bridge, {
              'kind': 'fileBoundaryCleanup',
              'run': run,
            }, directory: root.path);
            expect(result['ok'], true);
          }
          expect(await root.list().toList(), isEmpty);
          await root.delete();
        }
      },
    );
  }
}
