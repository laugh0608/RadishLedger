import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/probe.dart';
import 'package:radishledger_probe/src/validation.dart';

void main() {
  for (final candidate in BridgeCandidate.values) {
    test(
      '${candidate.name} native faults, ownership pressure and shared database',
      () async {
        final result = await runBridgePressure(candidate);
        expect(result['liveBuffers'], 0);
        expect(result['calls'], 6200);
        expect((result['concurrent']! as Map)['committed'], 65);
        expect((result['concurrent']! as Map)['replayed'], 71);
      },
    );
  }
}
