// 人工实验页的非秘密归属记录；不用于生产账库或跨设备恢复。
import 'dart:convert';
import 'dart:io';
import 'bootstrap.dart';
import 'probe.dart';

class BootstrapSelection {
  BootstrapSelection(this.run, this.bridge, {this.cleanupCandidates});
  final String run;
  final BridgeCandidate bridge;
  final List<String>? cleanupCandidates;

  static bool validIdentity(Object? value) =>
      value is String && RegExp(r'^[0-9a-f]{32}$').hasMatch(value);

  static List<String> generations(Object? value) {
    if (value is! List ||
        value.length > 256 ||
        !value.every(validIdentity) ||
        value.toSet().length != value.length) {
      throw const BootstrapFailure('invalid_selection');
    }
    return List<String>.unmodifiable(value.cast<String>());
  }

  static Future<BootstrapSelection?> load(String directory) async {
    final path = '$directory/selection.json';
    // 未完成的发布不能当成无账库。保留原记录和 pending，等待精确核对。
    if (await FileSystemEntity.type('$path.pending', followLinks: false) !=
        FileSystemEntityType.notFound) {
      throw const BootstrapFailure('selection_publish_incomplete');
    }
    final type = await FileSystemEntity.type(path, followLinks: false);
    if (type == FileSystemEntityType.notFound) return null;
    if (type != FileSystemEntityType.file) {
      throw const BootstrapFailure('invalid_selection');
    }
    final file = File(path);
    if (await file.length() > 16384) {
      throw const BootstrapFailure('invalid_selection');
    }
    try {
      final value = jsonDecode(await file.readAsString());
      if (value is! Map<String, dynamic> ||
          !validIdentity(value['run']) ||
          !value.keys.every(
            (key) => ['run', 'bridge', 'cleanupCandidates'].contains(key),
          ) ||
          !BridgeCandidate.values.any((b) => b.name == value['bridge'])) {
        throw const BootstrapFailure('invalid_selection');
      }
      return BootstrapSelection(
        value['run'] as String,
        BridgeCandidate.values.byName(value['bridge'] as String),
        cleanupCandidates: value.containsKey('cleanupCandidates')
            ? generations(value['cleanupCandidates'])
            : null,
      );
    } on FormatException {
      throw const BootstrapFailure('invalid_selection');
    }
  }

  void requireOwner(BootstrapService service) {
    if (run != service.run || bridge != service.bridge) {
      throw const BootstrapFailure('selection_changed');
    }
  }

  Future<void> save(String directory, {bool create = false}) async {
    if (!validIdentity(run)) {
      throw const BootstrapFailure('invalid_selection');
    }
    final candidates = cleanupCandidates;
    if (candidates != null) generations(candidates);
    final current = await load(directory);
    if (create ? current != null : current == null) {
      throw const BootstrapFailure('selection_changed');
    }
    if (current != null && (current.run != run || current.bridge != bridge)) {
      throw const BootstrapFailure('selection_changed');
    }
    final path = '$directory/selection.json';
    final pending = File('$path.pending');
    await pending.create(exclusive: true);
    await pending.writeAsString(
      jsonEncode({
        'run': run,
        'bridge': bridge.name,
        'cleanupCandidates': ?candidates,
      }),
      flush: true,
    );
    await pending.rename(path);
  }
}
