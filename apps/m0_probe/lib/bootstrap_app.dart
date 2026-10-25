import 'dart:convert';
import 'dart:math';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'src/bootstrap.dart';
import 'src/bootstrap_selection.dart';
import 'src/probe.dart';

class BootstrapApp extends StatelessWidget {
  const BootstrapApp({super.key, this.service});
  final BootstrapService? service;
  @override
  Widget build(BuildContext context) => MaterialApp(
    title: '萝卜账簿 · 解锁实验',
    theme: ThemeData(
      colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xff435c74)),
      scaffoldBackgroundColor: const Color(0xfff7f4ee),
      inputDecorationTheme: const InputDecorationTheme(
        border: OutlineInputBorder(),
      ),
    ),
    home: BootstrapPage(service: service),
  );
}

class BootstrapPage extends StatefulWidget {
  const BootstrapPage({super.key, this.service});
  final BootstrapService? service;
  @override
  State<BootstrapPage> createState() => _BootstrapPageState();
}

class _BootstrapPageState extends State<BootstrapPage>
    with WidgetsBindingObserver {
  final _password = TextEditingController();
  final _recovery = TextEditingController();
  BootstrapService? _service;
  BridgeCandidate _bridge = BridgeCandidate.thin;
  bool _busy = false;
  bool _hasVault = false;
  bool _cleanupPending = false;
  bool _shield = false;
  int _epoch = 0;
  Uint8List? _issuedRecovery;
  String _status = '正在核对实验目录';
  String? _error;
  Map<String, dynamic>? _report;
  Map<String, dynamic>? _capability;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _initialize();
  }

  String _newRun() {
    final random = Random.secure();
    return List.generate(
      16,
      (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
    ).join();
  }

  Future<void> _initialize() async {
    try {
      final platform = widget.service?.platform ?? SystemProtection();
      final directory = widget.service?.directory ?? await platform.storage();
      var run = widget.service?.run ?? _newRun();
      _bridge = widget.service?.bridge ?? _bridge;
      final selection = await BootstrapSelection.load(directory);
      if (selection != null) {
        if (widget.service != null) selection.requireOwner(widget.service!);
        run = selection.run;
        _bridge = selection.bridge;
        _cleanupPending = selection.cleanupCandidates != null;
      }
      _hasVault = selection != null || widget.service != null;
      final capability = await platform.capabilities();
      if (!mounted) return;
      setState(() {
        _service =
            widget.service ??
            BootstrapService(directory, run, _bridge, platform);
        _capability = capability;
        _status = _cleanupPending
            ? '上次清理尚未完成，请重试本次清理；不能再解锁或创建'
            : _hasVault
            ? '已找到原实验账库，请解锁或继续创建'
            : '仅用于合成材料；创建后请保存恢复材料';
      });
    } catch (_) {
      if (mounted) {
        setState(() {
          _error = '实验目录或状态不可读，已有材料保留。';
        });
      }
    }
  }

  void _clearIssued() {
    final bytes = _issuedRecovery;
    bytes?.fillRange(0, bytes.length, 0);
    _issuedRecovery = null;
  }

  Uint8List _passwordBytes() {
    final bytes = Uint8List.fromList(utf8.encode(_password.text));
    _password.clear();
    return bytes;
  }

  Uint8List _recoveryBytes() {
    final bytes = Uint8List.fromList(utf8.encode(_recovery.text));
    _recovery.clear();
    return bytes;
  }

  String _message(String code) => switch (code) {
    'authentication_failed' => '解锁材料不正确或封装损坏，原账库保留。',
    'invalid_recovery' => '恢复材料格式或校验不正确，请检查输入。',
    'invalid_password' => '请输入有效口令。',
    'unsupported' => '当前设备不支持要求的系统认证保护；可继续使用独立口令或恢复材料。',
    'item_missing' ||
    'item_invalidated' ||
    'protection_missing' ||
    'material_missing' => '日常保护不可用，请用独立材料读取原账库后重建保护。',
    'user_cancelled' => '已取消系统认证，原账库和独立恢复材料保留。',
    'authentication_denied' => '系统认证被拒绝，请稍后重试或使用独立恢复。',
    'temporarily_unavailable' ||
    'authentication_unavailable' => '系统保护暂不可取，请稍后重试。',
    'interrupted' => '已锁定；未完成操作的结果需重新核对。',
    'already_exists' => '已有创建材料，请继续原创建流程。',
    'recovery_unconfirmed' => '请先输入已保存的恢复材料并确认。',
    'already_ready' => '账库已确认；不能重新生成初始恢复材料。',
    'missing_database' ||
    'invalid_bootstrap' ||
    'storage_error' => '原材料不完整或不可读，已保留文件，请勿新建覆盖。',
    'kdf_busy' || 'vault_busy' || 'protection_busy' => '已有操作尚未结束，请稍后再试。',
    'selection_changed' => '本次实验归属已变化，已停止操作，请重新打开核对。',
    'invalid_selection' ||
    'selection_publish_incomplete' => '本次归属登记损坏或发布未完成，保留现场，请勿新建覆盖。',
    'cleanup_failed' => '清理尚未完成，保留本次登记，请重试精确清理。',
    _ => '操作未完成（$code），原材料保留。',
  };

  Future<void> _perform(
    Future<BootstrapResult> Function() action, {
    bool refreshSelection = false,
  }) async {
    if (_busy || _service == null) return;
    FocusManager.instance.primaryFocus?.unfocus();
    final epoch = ++_epoch;
    setState(() {
      _busy = true;
      _error = null;
      _report = null;
      _status = '正在处理，请稍候';
    });
    BootstrapResult? result;
    try {
      result = await action();
      result.requireSuccess();
      if (!mounted || epoch != _epoch) return;
      setState(() {
        _report = result!.report;
        if (result.secret.length == 80) {
          _clearIssued();
          _issuedRecovery = Uint8List.fromList(result.secret);
        }
        _status = result.report['cleaned'] == true
            ? '本次合成实验已清理，可以选择另一接入候选'
            : result.report['actionCancelled'] == true
            ? '已取消实验操作，原材料保留'
            : result.report['protectionRemoved'] != null
            ? '本次日常保护项已删除；请验证系统解锁失败，再用独立材料读取原账库'
            : result.report['fixtureComplete'] == true
            ? '原账库核对成功，未同步操作 ${(result.report['audit'] as Map)['operations'].length} 条'
            : result.report['ready'] == true
            ? '恢复材料已确认，可以写入合成账务'
            : '创建材料已保存，请确认独立恢复材料';
      });
    } on BootstrapFailure catch (error) {
      if (mounted && epoch == _epoch) {
        setState(() {
          _error = _message(error.code);
          _status = '操作未完成';
        });
      }
    } catch (_) {
      if (mounted && epoch == _epoch) {
        setState(() {
          _error = '结果不可确定，请重新解锁核对原账库。';
          _status = '需要重新核对';
        });
      }
    } finally {
      result?.clear();
      if (refreshSelection && mounted) await _refreshSelection();
      if (mounted) {
        _password.clear();
        _recovery.clear();
        setState(() {
          _busy = false;
        });
      }
    }
  }

  Future<BootstrapResult> _create() async {
    final service = _service!;
    final password = _passwordBytes();
    BootstrapResult? generated;
    try {
      // 与既有核心输入上限一致；拒绝输入时不要先留下无法继续的 selection。
      if (password.isEmpty || password.length > 1024) {
        throw const BootstrapFailure('invalid_password');
      }
      generated = await service.core({'kind': 'newRecovery'}, Uint8List(0));
      generated.requireSuccess();
      await BootstrapSelection(
        service.run,
        _bridge,
      ).save(service.directory, create: true);
      _hasVault = true;
      final input = Uint8List(password.length + generated.secret.length)
        ..setAll(0, password)
        ..setAll(password.length, generated.secret);
      final result = await service.core({'kind': 'create'}, input);
      result.requireSuccess();
      return BootstrapResult(
        result.status,
        Uint8List.fromList(generated.secret),
      );
    } finally {
      password.fillRange(0, password.length, 0);
      generated?.clear();
    }
  }

  Future<void> _refreshSelection() async {
    final service = _service!;
    try {
      final selection = await BootstrapSelection.load(service.directory);
      if (!mounted) return;
      if (selection != null) selection.requireOwner(service);
      setState(() {
        _hasVault = selection != null;
        _cleanupPending = selection?.cleanupCandidates != null;
        if (selection == null) {
          _clearIssued();
          _report = null;
          _service = BootstrapService(
            service.directory,
            _newRun(),
            _bridge,
            service.platform,
          );
          _status = '本次合成实验已清理，可以选择另一接入候选';
        }
      });
    } catch (_) {
      if (mounted) {
        setState(() {
          _service = null;
          _error = '无法核对实验归属，已停止操作并保留现场。';
        });
      }
    }
  }

  Future<bool> _confirmExperiment(String title, String detail) async =>
      await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text(title),
          content: Text(detail),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('取消'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('确认本次操作'),
            ),
          ],
        ),
      ) ??
      false;

  BootstrapResult _cancelledAction() => BootstrapResult({
    'ok': true,
    'report': {'actionCancelled': true},
  }, Uint8List(0));

  Future<void> _removeProtection() => _perform(() async {
    final service = _service!;
    final epoch = _epoch;
    final generation = await service.activeProtection();
    if (!mounted || epoch != _epoch) {
      throw const BootstrapFailure('interrupted');
    }
    final confirmed = await _confirmExperiment(
      '使本次日常保护项缺失？',
      '仅删除本次系统保护项，保留原账库和独立恢复封装。请先确保独立材料可用。\n'
          '实验：${service.run}\n保护项：$generation',
    );
    if (!confirmed) return _cancelledAction();
    if (epoch != _epoch) throw const BootstrapFailure('interrupted');
    return service.removeActiveProtection(generation);
  });

  Future<void> _cleanup() => _perform(() async {
    final service = _service!;
    final epoch = _epoch;
    if (!_cleanupPending) {
      final confirmed = await _confirmExperiment(
        '清理本次合成实验？',
        '将删除本次合成账务、未同步操作、候选、恢复封装及本次全部日常保护项，无法撤回。\n'
            '仅限 ${service.bridge.name} 实验：${service.run}',
      );
      if (!confirmed) return _cancelledAction();
    }
    if (epoch != _epoch) throw const BootstrapFailure('interrupted');
    return service.cleanupSelection();
  }, refreshSelection: true);

  Future<void> _lock() async {
    FocusManager.instance.primaryFocus?.unfocus();
    _epoch += 1;
    _password.clear();
    _recovery.clear();
    _clearIssued();
    if (mounted) {
      setState(() {
        _report = null;
        _error = null;
        _status = _busy ? '已停止等待；核心结束前不能开始新操作' : '已锁定，请重新输入解锁材料';
      });
    }
    try {
      await _service?.lock();
    } catch (_) {
      if (mounted) {
        setState(() {
          _error = '系统取消未得到确认，保持锁定并等待原操作结束。';
        });
      }
    }
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) {
      if (mounted) {
        setState(() {
          _shield = false;
        });
      }
    } else {
      if (mounted) {
        setState(() {
          _shield = true;
        });
      }
      if (state == AppLifecycleState.paused ||
          state == AppLifecycleState.hidden ||
          state == AppLifecycleState.detached) {
        _lock();
      }
    }
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _epoch += 1;
    _clearIssued();
    _password.clear();
    _recovery.clear();
    _password.dispose();
    _recovery.dispose();
    _service?.lock().catchError((Object _) {
      debugPrint('M0C_CANCEL_NOT_CONFIRMED');
    });
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_shield) return const Scaffold(body: Center(child: Text('已锁定')));
    final enabled = !_busy && _service != null;
    final vaultEnabled = enabled && !_cleanupPending;
    return Scaffold(
      appBar: AppBar(
        title: const Text('萝卜账簿 · 解锁实验'),
        actions: [
          IconButton(
            onPressed: _lock,
            icon: const Icon(Icons.lock_outline),
            tooltip: '锁定',
          ),
        ],
      ),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.all(20),
          children: [
            const Text(
              '本机解锁与独立恢复',
              style: TextStyle(fontSize: 24, fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 12),
            const Text('只使用合成账务。恢复材料用于读取原数据，不是完整备份。'),
            const SizedBox(height: 16),
            Text(
              _capability?['supported'] == true
                  ? '系统认证保护可用，实际保护等级仍需真机验证。'
                  : '本设备尚不具备要求的系统认证条件，独立解锁仍可验证。',
            ),
            const SizedBox(height: 16),
            DropdownButtonFormField<BridgeCandidate>(
              initialValue: _bridge,
              decoration: const InputDecoration(labelText: '接入候选'),
              items: BridgeCandidate.values
                  .map(
                    (value) =>
                        DropdownMenuItem(value: value, child: Text(value.name)),
                  )
                  .toList(),
              onChanged: enabled && !_hasVault
                  ? (value) {
                      if (value != null) {
                        setState(() {
                          _bridge = value;
                          _service = BootstrapService(
                            _service!.directory,
                            _service!.run,
                            value,
                            _service!.platform,
                          );
                        });
                      }
                    }
                  : null,
            ),
            const SizedBox(height: 16),
            TextField(
              key: const ValueKey('m0c-password'),
              controller: _password,
              obscureText: true,
              autocorrect: false,
              enableSuggestions: false,
              decoration: const InputDecoration(labelText: '独立解锁口令'),
              enabled: vaultEnabled,
            ),
            const SizedBox(height: 12),
            TextField(
              key: const ValueKey('m0c-recovery'),
              controller: _recovery,
              obscureText: true,
              autocorrect: false,
              enableSuggestions: false,
              decoration: const InputDecoration(labelText: '已保存的恢复材料'),
              enabled: vaultEnabled,
            ),
            if (_issuedRecovery != null) ...[
              const SizedBox(height: 12),
              const Text('请保存以下恢复材料，再重新输入确认。不会自动复制。'),
              Text(
                utf8.decode(_issuedRecovery!),
                key: const ValueKey('m0c-issued'),
              ),
              TextButton(
                onPressed: () {
                  setState(_clearIssued);
                },
                child: const Text('隐藏恢复材料'),
              ),
            ],
            const SizedBox(height: 16),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                if (!_hasVault)
                  FilledButton(
                    key: const ValueKey('m0c-create'),
                    onPressed: enabled ? () => _perform(_create) : null,
                    child: const Text('创建合成账库'),
                  ),
                if (_hasVault && !_cleanupPending) ...[
                  FilledButton(
                    key: const ValueKey('m0c-unlock-password'),
                    onPressed: enabled
                        ? () => _perform(
                            () => _service!.core({
                              'kind': 'inspect',
                              'unlock': 'password',
                            }, _passwordBytes()),
                          )
                        : null,
                    child: const Text('口令解锁'),
                  ),
                  OutlinedButton(
                    key: const ValueKey('m0c-unlock-recovery'),
                    onPressed: enabled
                        ? () => _perform(
                            () => _service!.core({
                              'kind': 'inspect',
                              'unlock': 'recovery',
                            }, _recoveryBytes()),
                          )
                        : null,
                    child: const Text('独立恢复'),
                  ),
                  OutlinedButton(
                    onPressed: enabled
                        ? () => _perform(
                            () => _service!.core({
                              'kind': 'resume',
                              'unlock': 'password',
                            }, _passwordBytes()),
                          )
                        : null,
                    child: const Text('继续创建'),
                  ),
                  OutlinedButton(
                    onPressed: enabled
                        ? () => _perform(
                            () => _service!.core({
                              'kind': 'reissueRecovery',
                            }, _passwordBytes()),
                          )
                        : null,
                    child: const Text('重发待确认恢复材料'),
                  ),
                  OutlinedButton(
                    onPressed: enabled
                        ? () => _perform(
                            () => _service!.core({
                              'kind': 'confirm',
                            }, _recoveryBytes()),
                          )
                        : null,
                    child: const Text('确认恢复材料'),
                  ),
                  OutlinedButton(
                    onPressed: enabled
                        ? () => _perform(
                            () => _service!.core({
                              'kind': 'seed',
                              'unlock': 'password',
                            }, _passwordBytes()),
                          )
                        : null,
                    child: const Text('写入合成账务'),
                  ),
                  OutlinedButton(
                    onPressed: enabled
                        ? () => _perform(
                            () => _service!.establishProtection(
                              'password',
                              _passwordBytes(),
                            ),
                          )
                        : null,
                    child: const Text('建立日常保护'),
                  ),
                  OutlinedButton(
                    onPressed: enabled
                        ? () => _perform(() => _service!.unlockSystem())
                        : null,
                    child: const Text('系统认证解锁'),
                  ),
                  OutlinedButton(
                    key: const ValueKey('m0c-rebuild-recovery'),
                    onPressed: enabled
                        ? () => _perform(
                            () => _service!.establishProtection(
                              'recovery',
                              _recoveryBytes(),
                            ),
                          )
                        : null,
                    child: const Text('用恢复材料重建日常保护'),
                  ),
                  OutlinedButton(
                    key: const ValueKey('m0c-remove-protection'),
                    onPressed: enabled ? _removeProtection : null,
                    child: const Text('实验：使日常保护项缺失'),
                  ),
                ],
                if (_hasVault)
                  OutlinedButton(
                    key: const ValueKey('m0c-cleanup'),
                    onPressed: enabled ? _cleanup : null,
                    child: Text(_cleanupPending ? '重试本次清理' : '清理本次合成实验'),
                  ),
                if (_busy)
                  TextButton(
                    key: const ValueKey('m0c-cancel'),
                    onPressed: _lock,
                    child: const Text('停止等待并锁定'),
                  ),
              ],
            ),
            const SizedBox(height: 20),
            if (_busy) const LinearProgressIndicator(),
            Text(_status, key: const ValueKey('m0c-status')),
            if (_report?['platformProtection']?['securityLevelCode'] != null)
              Text(
                '系统返回安全等级代码：${_report!['platformProtection']['securityLevelCode']}（尚未经真机核验）',
              ),
            if (_error != null)
              Text(
                _error!,
                key: const ValueKey('m0c-error'),
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
            if (_report?['fixtureComplete'] == true)
              const Text('完整事实与未同步队列已核对', key: ValueKey('m0c-success')),
          ],
        ),
      ),
    );
  }
}
