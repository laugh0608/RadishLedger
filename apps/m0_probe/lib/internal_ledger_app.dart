import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import 'src/bootstrap.dart';
import 'src/internal_ledger.dart';
import 'src/local_book.dart';

class InternalLedgerApp extends StatelessWidget {
  const InternalLedgerApp({
    super.key,
    this.platform = const SystemProtection(),
  });

  final SystemProtection platform;

  @override
  Widget build(BuildContext context) => MaterialApp(
    title: '萝卜账簿',
    theme: ThemeData(
      colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xff435c74)),
      inputDecorationTheme: const InputDecorationTheme(
        border: OutlineInputBorder(),
      ),
    ),
    home: _LedgerPage(platform),
  );
}

class _LedgerPage extends StatefulWidget {
  const _LedgerPage(this.platform);
  final SystemProtection platform;

  @override
  State<_LedgerPage> createState() => _LedgerPageState();
}

class _LedgerPageState extends State<_LedgerPage> with WidgetsBindingObserver {
  final _input = TextEditingController();
  InternalLedger? _ledger;
  InternalLedgerLocation? _location;
  bool _deviceAuthentication = false;
  bool _busy = true;
  LocalBook? _book;
  bool _pendingConfirmation = false;
  bool _reissuing = false;
  LedgerUnlock _method = LedgerUnlock.password;
  Uint8List? _issued;
  String? _message;
  int _epoch = 0;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _load();
  }

  Future<void> _load() async {
    try {
      final ledger = await InternalLedger.open(widget.platform);
      final location = await ledger.locate();
      final capabilities = await widget.platform.capabilities();
      if (!mounted) return;
      setState(() {
        _ledger = ledger;
        _location = location;
        _deviceAuthentication = capabilities['supported'] == true;
        _message = null;
        _busy = false;
      });
    } catch (error) {
      if (mounted) {
        setState(() {
          _message = _explain(error);
          _busy = false;
        });
      }
    }
  }

  String _explain(Object error) => switch (error) {
    BootstrapFailure(code: 'authentication_failed') =>
      '口令或恢复材料不正确，也可能是封装损坏。原账簿已保留。',
    BootstrapFailure(code: 'invalid_recovery') => '恢复材料格式或校验有误，请检查后重试。',
    BootstrapFailure(code: 'invalid_password') => '请输入有效的本地口令。',
    BootstrapFailure(code: 'vault_busy' || 'kdf_busy') => '上次操作尚未结束，请稍后重试。',
    BootstrapFailure(code: 'already_exists' || 'selection_changed') =>
      '账簿已存在或身份有变化，请重新核对原账簿。',
    BootstrapFailure(
      code: 'bootstrap_publish_uncertain' || 'invalid_response',
    ) =>
      '结果未定，请重新核对原账簿。',
    BootstrapFailure(code: 'local_save_uncertain') => '保存结果未定，请重新打开账簿核对上次保存。',
    BootstrapFailure(code: 'stale_revision') => '这笔流水已变化。请重新解锁，核对最新记录后再编辑。',
    BootstrapFailure(code: 'stale_candidates') => '这笔流水存在冲突。请重新解锁核对，当前修改未保存。',
    BootstrapFailure(
      code: 'missing_database' ||
          'unexpected_database' ||
          'storage_error' ||
          'invalid_bootstrap' ||
          'bootstrap_io' ||
          'foreign_directory',
    ) =>
      '原账簿或创建材料不完整、损坏或不可读。文件已保留，请重新核对。',
    BootstrapFailure(message: final message?) => message,
    BootstrapFailure(code: final code) => '操作未完成（$code）。请重新核对。',
    _ => '无法核对本地账簿，请重试。',
  };

  void _clearIssued() {
    final bytes = _issued;
    bytes?.fillRange(0, bytes.length, 0);
    _issued = null;
  }

  void _lock() {
    _epoch++;
    _input.clear();
    _clearIssued();
    FocusManager.instance.primaryFocus?.unfocus();
    setState(() {
      _book = null;
      _pendingConfirmation = false;
      _reissuing = false;
      _method = LedgerUnlock.password;
      _message = _busy ? '已停止等待，核心结束后会核对原账簿。' : null;
    });
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state != AppLifecycleState.resumed) _lock();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _clearIssued();
    _input.clear();
    _input.dispose();
    super.dispose();
  }

  Future<bool> _submit(
    Future<BootstrapResult> Function(Uint8List) action, {
    required bool canWrite,
    bool saving = false,
  }) async {
    if (_busy) return false;
    final epoch = ++_epoch;
    final input = Uint8List.fromList(utf8.encode(_input.text));
    _input.clear();
    FocusManager.instance.primaryFocus?.unfocus();
    setState(() {
      _message = null;
      _busy = true;
    });
    BootstrapResult? result;
    var accepted = false;
    try {
      result = await action(input);
      if (!mounted || epoch != _epoch) return false;
      final identity = InternalLedger.identity(result.report);
      final ready = result.report['ready'] == true;
      final book = ready ? LocalBook.fromReport(result.report) : null;
      if (saving &&
          (book?.lastSaveCommitted != true || result.secret.isNotEmpty)) {
        throw const BootstrapFailure('invalid_response');
      }
      if (result.secret.isNotEmpty && result.secret.length != 80) {
        throw const BootstrapFailure('invalid_response');
      }
      final released = result.secret.isEmpty
          ? null
          : Uint8List.fromList(result.secret);
      setState(() {
        _location = (state: InternalLedgerState.registered, identity: identity);
        _book = book;
        _pendingConfirmation = !ready;
        _reissuing = false;
        if (released != null || ready) _clearIssued();
        _issued ??= released;
        if (saving) {
          _message = '已保存到本机';
        } else if (book?.lastSaveCommitted case final committed?) {
          _message = committed ? '最近核对的保存已提交。' : '最近核对的保存未提交，可重新录入。';
        }
      });
      accepted = true;
    } catch (error) {
      if (mounted && epoch == _epoch) {
        setState(() {
          final uncertain =
              saving &&
              (error is! BootstrapFailure ||
                  const {
                    'local_save_uncertain',
                    'bootstrap_publish_uncertain',
                    'internal_error',
                    'invalid_output',
                    'invalid_response',
                    'selection_changed',
                  }.contains(error.code));
          if (uncertain ||
              error is BootstrapFailure &&
                  (error.code == 'stale_revision' ||
                      error.code == 'stale_candidates')) {
            _book = null;
          }
          if (error is BootstrapFailure &&
              (error.code == 'bootstrap_publish_uncertain' ||
                  error.code == 'selection_changed' ||
                  error.code == 'invalid_response')) {
            _pendingConfirmation = false;
            _reissuing = false;
            _clearIssued();
            if (!canWrite) _location = null;
          }
          _message =
              uncertain &&
                  (error is! BootstrapFailure ||
                      error.code != 'selection_changed')
              ? _explain(const BootstrapFailure('local_save_uncertain'))
              : _explain(error);
        });
      }
    } finally {
      result?.clear();
      input.fillRange(0, input.length, 0);
      if (((canWrite && !saving) ||
              (saving && _book == null) ||
              epoch != _epoch) &&
          !accepted &&
          mounted &&
          _ledger != null) {
        try {
          final location = await _ledger!.locate();
          final previous = _location?.identity;
          if (previous != null && previous != location.identity) {
            throw const BootstrapFailure('selection_changed');
          }
          if (mounted) setState(() => _location = location);
        } catch (error) {
          if (mounted) {
            setState(() {
              _location = null;
              _pendingConfirmation = false;
              _clearIssued();
              _message = _explain(error);
            });
          }
        }
      }
      if (mounted) setState(() => _busy = false);
    }
    return accepted;
  }

  void _retry() {
    setState(() {
      _message = null;
      _busy = true;
    });
    _load();
  }

  void _changeMethod() {
    _input.clear();
    setState(() {
      _message = null;
      if (_pendingConfirmation) {
        _reissuing = !_reissuing;
      } else {
        _method = _method == LedgerUnlock.password
            ? LedgerUnlock.recovery
            : LedgerUnlock.password;
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final ledger = _ledger;
    final location = _location;
    final book = _book;
    return Scaffold(
      appBar: AppBar(title: const Text('萝卜账簿')),
      body: SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const Text('仅用于合成数据'),
              const SizedBox(height: 16),
              if (book != null &&
                  ledger != null &&
                  location?.identity != null) ...[
                Text('账簿已打开', style: Theme.of(context).textTheme.headlineSmall),
                const SizedBox(height: 16),
                FilledButton(onPressed: _lock, child: const Text('关闭账簿')),
              ],
              if (_busy) ...[
                const SizedBox(height: 12),
                const LinearProgressIndicator(semanticsLabel: '正在处理本地账簿'),
                if (_location != null)
                  TextButton(onPressed: _lock, child: const Text('停止等待')),
              ],
              if (book == null && _message != null) ...[
                const SizedBox(height: 12),
                Semantics(liveRegion: true, child: Text(_message!)),
              ],
              if (book != null && ledger != null && location?.identity != null)
                LocalBookPanel(
                  book: book,
                  unlock: _method,
                  credential: _input,
                  busy: _busy,
                  message: _message,
                  clearMessage: () => setState(() => _message = null),
                  save: (command) => _submit(
                    (input) => ledger.save(
                      location!.identity!,
                      _method,
                      input,
                      command,
                    ),
                    canWrite: true,
                    saving: true,
                  ),
                )
              else if (ledger != null && location != null)
                ..._form(context, ledger, location)
              else if (!_busy)
                OutlinedButton(onPressed: _retry, child: const Text('重新核对')),
            ],
          ),
        ),
      ),
    );
  }

  List<Widget> _form(
    BuildContext context,
    InternalLedger ledger,
    InternalLedgerLocation location,
  ) {
    final empty = location.state == InternalLedgerState.empty;
    final initializing = location.state == InternalLedgerState.initializing;
    final recovery = _pendingConfirmation
        ? !_reissuing
        : _method == LedgerUnlock.recovery;
    final title = empty
        ? '创建本地账簿'
        : _pendingConfirmation
        ? '确认恢复材料'
        : initializing
        ? '继续创建账簿'
        : '解锁本地账簿';
    final identity = location.identity;
    final (label, action, canWrite) = empty
        ? (
            '创建账簿',
            (Uint8List input) => ledger.create(input, includeLedger: true),
            true,
          )
        : _pendingConfirmation
        ? _reissuing
              ? (
                  '重新生成恢复材料',
                  (Uint8List input) => ledger.reissue(identity!, input),
                  true,
                )
              : (
                  '确认已保存的恢复材料',
                  (Uint8List input) =>
                      ledger.confirm(identity!, input, includeLedger: true),
                  true,
                )
        : (
            initializing ? '继续创建' : '打开账簿',
            (Uint8List input) =>
                ledger.unlock(location, _method, input, includeLedger: true),
            initializing,
          );
    final issued = _issued;
    return [
      Text(title, style: Theme.of(context).textTheme.headlineSmall),
      if (!_deviceAuthentication && !empty && !_pendingConfirmation)
        const Text('设备认证不可用。可使用本地口令或独立恢复材料。'),
      if (_pendingConfirmation && issued != null && !_reissuing) ...[
        const SizedBox(height: 12),
        const Text('请离线保存这份恢复材料，再输入已保存的内容确认。'),
        SelectableText(
          utf8.decode(issued),
          style: const TextStyle(fontFamily: 'monospace'),
        ),
      ],
      if (_reissuing) const Text('重新生成后，上一份尚未确认的恢复材料失效。'),
      const SizedBox(height: 16),
      TextField(
        controller: _input,
        enabled: !_busy,
        obscureText: true,
        enableSuggestions: false,
        autocorrect: false,
        enableIMEPersonalizedLearning: false,
        decoration: InputDecoration(labelText: recovery ? '恢复材料' : '本地口令'),
      ),
      const SizedBox(height: 12),
      FilledButton(
        onPressed: _busy ? null : () => _submit(action, canWrite: canWrite),
        child: Text(label),
      ),
      if (!empty)
        TextButton(
          onPressed: _busy ? null : _changeMethod,
          child: Text(
            _pendingConfirmation
                ? _reissuing
                      ? '返回确认恢复材料'
                      : '使用口令重新生成'
                : recovery
                ? '改用本地口令'
                : '改用恢复材料',
          ),
        ),
    ];
  }
}
