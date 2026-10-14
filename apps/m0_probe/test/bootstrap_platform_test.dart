import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:radishledger_probe/src/bootstrap.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  tearDown(
    () => messenger.setMockMethodCallHandler(SystemProtection.channel, null),
  );
  test('system codec result becomes an owned mutable buffer', () async {
    messenger.setMockMethodCallHandler(
      SystemProtection.channel,
      (_) async => Uint8List.fromList(List.filled(32, 19)).asUnmodifiableView(),
    );
    final secret = await SystemProtection().read('synthetic', 'synthetic');
    expect(secret.length, 32);
    secret.fillRange(0, secret.length, 0);
    expect(secret.every((value) => value == 0), isTrue);
  });
  test(
    'platform cancellation retains structured code and clears create input',
    () async {
      messenger.setMockMethodCallHandler(
        SystemProtection.channel,
        (_) async => throw PlatformException(
          code: 'user_cancelled',
          details: {'platformCode': -128},
        ),
      );
      final input = Uint8List.fromList(List.filled(32, 19));
      await expectLater(
        SystemProtection().create('synthetic', 'synthetic', input),
        throwsA(
          isA<BootstrapFailure>()
              .having((e) => e.code, 'code', 'user_cancelled')
              .having((e) => e.platformCode, 'platformCode', -128),
        ),
      );
      expect(input.every((value) => value == 0), isTrue);
    },
  );
}
