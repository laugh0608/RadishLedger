import 'dart:io';
import 'package:code_assets/code_assets.dart';
import 'package:hooks/hooks.dart';

Future<void> main(List<String> args) async {
  await build(args, (input, output) async {
    if (!input.config.buildCodeAssets) return;
    final config = input.config.code;
    final root = input.packageRoot.resolve('../../');
    final destination = input.outputDirectory.resolve(
      config.targetOS == OS.android || config.targetOS == OS.linux
          ? 'libradishledger_mobile.so'
          : 'libradishledger_mobile.dylib',
    );
    final command = <String>[
      root.resolve('scripts/build-mobile-core.py').toFilePath(),
      '--os',
      config.targetOS.name,
      '--arch',
      config.targetArchitecture.name,
      '--output',
      destination.toFilePath(),
      '--frb',
    ];
    if (config.targetOS == OS.iOS) {
      command.addAll([
        '--ios-sdk',
        config.iOS.targetSdk.type,
        '--minimum-version',
        config.iOS.targetVersion.toString(),
      ]);
    }
    if (config.targetOS == OS.android) {
      command.addAll([
        '--minimum-version',
        config.android.targetNdkApi.toString(),
        '--c-compiler',
        config.cCompiler!.compiler.toFilePath(),
      ]);
    }
    final process = await Process.start(
      'python3',
      command,
      workingDirectory: root.toFilePath(),
      mode: ProcessStartMode.inheritStdio,
    );
    final status = await process.exitCode;
    if (status != 0) throw StateError('Rust 目标构建失败，exit=$status');
    output.assets.code.add(
      CodeAsset(
        package: input.packageName,
        name: 'src/native.dart',
        linkMode: DynamicLoadingBundled(),
        file: destination,
      ),
    );
    for (final path in [
      'Cargo.toml',
      'Cargo.lock',
      'rust-toolchain.toml',
      'scripts/build-mobile-core.py',
      'docs/validation/accounting-cases.json',
    ]) {
      output.dependencies.add(root.resolve(path));
    }
    for (final entity in Directory.fromUri(
      root.resolve('crates/'),
    ).listSync(recursive: true).whereType<File>()) {
      if (entity.path.endsWith('.rs') || entity.path.endsWith('Cargo.toml')) {
        output.dependencies.add(entity.uri);
      }
    }
    // 本地上游源码不是 workspace 成员，但仍是 native code asset 的真实输入。
    // 跟踪完整目录，避免仅改 C / 汇编 / build 源码时复用旧产物。
    for (final path in [
      'third-party/openssl-sys/',
      'third-party/openssl-src/',
    ]) {
      for (final entity in Directory.fromUri(
        root.resolve(path),
      ).listSync(recursive: true).whereType<File>()) {
        output.dependencies.add(entity.uri);
      }
    }
  });
}
