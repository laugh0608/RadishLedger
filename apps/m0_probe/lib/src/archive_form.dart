import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'internal_ledger.dart';

typedef SaveArchive = Future<String?> Function(String, Uint8List);

class ArchiveForm extends StatefulWidget {
  const ArchiveForm({
    super.key,
    required this.credential,
    required this.unlock,
    required this.busy,
    required this.message,
    required this.save,
    required this.leave,
  });
  final TextEditingController credential;
  final LedgerUnlock unlock;
  final bool busy;
  final String? message;
  final SaveArchive save;
  final VoidCallback leave;
  @override
  State<ArchiveForm> createState() => _ArchiveFormState();
}

class _ArchiveFormState extends State<ArchiveForm> {
  final name = TextEditingController();
  final password = TextEditingController();
  final repeat = TextEditingController();
  String? saved;
  String? error;

  @override
  void dispose() {
    name.dispose();
    password.clear();
    repeat.clear();
    password.dispose();
    repeat.dispose();
    super.dispose();
  }

  Future<void> submit() async {
    final target = name.text.trim();
    if (!RegExp(r'^[\p{L}\p{N}_-]{1,48}$', unicode: true).hasMatch(target)) {
      setState(() => error = '文件名限 1–48 个字母、汉字、数字、短横线或下划线。');
      return;
    }
    if (password.text != repeat.text || password.text.isEmpty) {
      setState(() => error = '请两次输入相同的归档口令。');
      return;
    }
    final bytes = Uint8List.fromList(utf8.encode(password.text));
    password.clear();
    repeat.clear();
    setState(() => error = null);
    try {
      final actual = await widget.save(target, bytes);
      if (mounted && actual != null) setState(() => saved = actual);
    } finally {
      bytes.fillRange(0, bytes.length, 0);
    }
  }

  Widget secretField(String label, TextEditingController controller) =>
      TextField(
        controller: controller,
        enabled: !widget.busy,
        obscureText: true,
        autocorrect: false,
        enableSuggestions: false,
        enableIMEPersonalizedLearning: false,
        decoration: InputDecoration(labelText: label),
      );

  @override
  Widget build(BuildContext context) => Column(
    crossAxisAlignment: CrossAxisAlignment.stretch,
    children: [
      if (saved != null) ...[
        Text('已回读认证并保存到 Download/RadishLedger/$saved'),
        TextButton(onPressed: widget.leave, child: const Text('返回账簿')),
      ] else ...[
        Text('新建加密归档', style: Theme.of(context).textTheme.titleMedium),
        const Text(
          '保存当前账簿的已提交记录及其历史，包括已删除的记录。文件位于 Download/RadishLedger，恢复时需要独立归档口令。',
        ),
        TextField(
          controller: name,
          enabled: !widget.busy,
          maxLength: 48,
          decoration: const InputDecoration(
            labelText: '新文件名',
            suffixText: '.rla',
          ),
        ),
        secretField(
          widget.unlock == LedgerUnlock.password ? '归档用本地口令' : '归档用恢复材料',
          widget.credential,
        ),
        secretField('独立归档口令', password),
        secretField('确认归档口令', repeat),
        if (error ?? widget.message case final message?)
          Semantics(liveRegion: true, child: Text(message)),
        FilledButton(
          onPressed: widget.busy ? null : submit,
          child: const Text('保存新归档'),
        ),
        TextButton(
          onPressed: widget.busy ? null : widget.leave,
          child: const Text('取消归档'),
        ),
      ],
    ],
  );
}
