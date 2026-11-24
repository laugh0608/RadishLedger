import 'dart:async';
import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'bootstrap.dart';
import 'internal_ledger.dart';
import 'restore_archive.dart';

class RestorePage extends StatefulWidget {
  const RestorePage({
    super.key,
    required this.ledger,
    required this.protectionAvailable,
  });
  final InternalLedger ledger;
  final bool protectionAvailable;
  @override
  State<RestorePage> createState() => _RestorePageState();
}

class _RestorePageState extends State<RestorePage> with WidgetsBindingObserver {
  final _password = TextEditingController();
  RestoreArchive? _session;
  Map<String, dynamic>? _report;
  bool _busy = false;
  bool _choosing = false;
  int _epoch = 0;
  String? _message;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed || _choosing) return;
    _stop(lock: true);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _epoch++;
    _password.clear();
    _password.dispose();
    // 正常返回在清理后才放行；引擎销毁时释放内存中的钥。
    if (!_busy) _session?.forgetKey();
    super.dispose();
  }

  String _explain(Object error) {
    final code = switch (error) {
      BootstrapFailure(:final code) => code,
      PlatformException(:final code) => code,
      _ => 'restore_io',
    };
    return switch (code) {
      'archive_unlock_failed' => '归档口令不正确或密钥封装已损坏。',
      'invalid_password' => '请输入有效的归档口令。',
      'archive_picker_unavailable' => '本机文件选择器不可用。',
      'archive_limit' || 'oversized_bootstrap' => '归档超过本次恢复支持的资源范围。',
      'invalid_archive' ||
      'unsupported_archive' ||
      'unsupported_kdf' => '归档格式无效或版本不受支持。',
      'archive_content_auth_failed' ||
      'invalid_archive_payload' => '归档内容未通过认证或关系核对。',
      'restore_mismatch' || 'restore_stage_missing' => '恢复输入或暂存库已变化，请保留现场核对。',
      _ => '恢复未完成（$code）。原归档和活动账簿已保留。',
    };
  }

  Future<void> _discard({bool releaseOnError = false}) async {
    final session = _session;
    if (session == null) return;
    try {
      await session.discard();
      _session = null;
      _message = '已清理本次恢复暂存。';
    } catch (_) {
      if (!mounted || releaseOnError) {
        session.forgetKey();
        _session = null;
      }
      if (mounted) {
        _message = releaseOnError
            ? '暂存清理未完成，已释放暂存钥。请重新选择原归档核对。'
            : '本次暂存清理未完成。请重试放弃，原归档和账簿已保留。';
      }
    }
  }

  Future<void> _run(Future<void> Function(int) action) async {
    if (_busy) return;
    final epoch = ++_epoch;
    setState(() {
      _busy = true;
      _message = null;
    });
    try {
      await action(epoch);
    } catch (error) {
      if (mounted && epoch == _epoch) _message = _explain(error);
    } finally {
      if (!mounted || epoch != _epoch) await _discard(releaseOnError: true);
      if (mounted) setState(() => _busy = false);
    }
  }

  void _stop({bool lock = false}) {
    _epoch++;
    _password.clear();
    _report = null;
    if (_busy) {
      setState(() => _message = '已停止等待，将清理本次暂存。');
    } else if (_session != null) {
      unawaited(_run((_) => _discard(releaseOnError: lock)));
    } else {
      setState(() => _message = null);
    }
  }

  Future<void> _choose() => _run((epoch) async {
    _choosing = true;
    bool? selected;
    try {
      selected = await archivePicker.invokeMethod<bool>('choose');
    } finally {
      _choosing = false;
    }
    if (!mounted || epoch != _epoch || selected != true) return;
    _session = await RestoreArchive.copySelection(widget.ledger);
  });

  Future<void> _review() => _run((epoch) async {
    _report = null;
    final password = Uint8List.fromList(utf8.encode(_password.text));
    _password.clear();
    FocusManager.instance.primaryFocus?.unfocus();
    final report = await _session!.review(password);
    if (mounted && epoch == _epoch) _report = report;
  });

  String _money(String minor) {
    final value = BigInt.parse(minor);
    final digits = value.abs().toString().padLeft(3, '0');
    return '${value.isNegative ? '-' : ''}¥${digits.substring(0, digits.length - 2)}.${digits.substring(digits.length - 2)}';
  }

  @override
  Widget build(BuildContext context) {
    final report = _report;
    return PopScope(
      canPop: !_busy && _session == null,
      onPopInvokedWithResult: (popped, _) {
        if (!popped && !_choosing) _stop();
      },
      child: Scaffold(
        appBar: AppBar(title: const Text('从归档恢复')),
        body: SafeArea(
          child: ListView(
            padding: const EdgeInsets.all(20),
            children: [
              if (report == null) ...[
                const Text('在独立空目标恢复并核对合成账务。离开页面或切到后台会清理暂存，原归档和本机账簿保留。'),
                const SizedBox(height: 12),
              ] else ...[
                Text(
                  '恢复暂存已核对',
                  style: Theme.of(context).textTheme.headlineSmall,
                ),
                Text('净资产 ${_money(report['netAssetsMinor'])}'),
                if ((report['accounts'] as int) > 8)
                  const Text('按账户标识列出前 8 个余额'),
                for (final row in (report['balances'] as List).cast<Map>())
                  Text('${row['name']} · ${_money(row['minor'])}'),
                Text(
                  '资产 ${_money(report['assetsMinor'])} · 负债 ${_money(report['liabilitiesMinor'])}',
                ),
                Text(
                  '账户 ${report['accounts']} · 分类 ${report['categories']} · 有效事件 ${report['activeEvents']}',
                ),
                Text('修订 ${report['revisions']} · 历史意图 ${report['intents']}'),
                Text(
                  '当前删除 ${report['deleted']} · 未决候选 ${report['candidates']}',
                ),
                Text('本机快照序号 ${report['sequence']}，其他设备的未同步范围未知。'),
                Text(
                  widget.protectionAvailable
                      ? '当前暂存仅供核对，尚未切换活动账簿。'
                      : '设备认证不可用，暂存不能切换为活动账簿。',
                ),
              ],
              if (_session == null)
                FilledButton(
                  onPressed: _busy ? null : _choose,
                  child: const Text('选择本机归档'),
                )
              else ...[
                const SizedBox(height: 12),
                TextField(
                  enabled: !_busy,
                  controller: _password,
                  obscureText: true,
                  autocorrect: false,
                  enableSuggestions: false,
                  enableIMEPersonalizedLearning: false,
                  decoration: const InputDecoration(labelText: '归档口令'),
                ),
                FilledButton(
                  onPressed: _busy ? null : _review,
                  child: Text(_session!.hasTarget ? '重新打开暂存核对' : '恢复到空暂存并核对'),
                ),
                TextButton(
                  onPressed: _busy ? null : _stop,
                  child: Text(_session!.hasTarget ? '放弃本次暂存' : '取消恢复'),
                ),
              ],
              if (_busy) ...[
                const LinearProgressIndicator(),
                if (!_choosing)
                  TextButton(onPressed: _stop, child: const Text('停止等待')),
              ],
              if (_message != null)
                Semantics(liveRegion: true, child: Text(_message!)),
            ],
          ),
        ),
      ),
    );
  }
}
