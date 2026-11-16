import 'dart:math';

import 'package:flutter/material.dart';

import 'bootstrap.dart';
import 'internal_ledger.dart';

class LocalBook {
  LocalBook._(
    this._accounts,
    this._categories,
    this.balances,
    this.entries,
    this.lastSaveCommitted,
    this.readOnly,
  );

  factory LocalBook.fromReport(Map<String, dynamic> report) {
    final book = report['book'];
    if (book is! Map<String, dynamic>) {
      throw const BootstrapFailure('invalid_response');
    }
    return LocalBook._(
      _references(book['accounts'] as List),
      _references(book['categories'] as List),
      Map<String, String>.from(book['balancesMinor'] as Map),
      (book['recentEntries'] as List).cast<Map<String, dynamic>>(),
      report['lastSaveCommitted'] as bool?,
      report['fixtureComplete'] == true,
    );
  }

  static Map<String, _Reference> _references(List rows) => {
    for (final row in rows.cast<Map<String, dynamic>>())
      row['id'] as String: (
        name: row['name'] as String,
        kind: row['kind'] as String,
        archived: row['archived'] as bool,
      ),
  };

  final Map<String, _Reference> _accounts;
  final Map<String, _Reference> _categories;
  final Map<String, String> balances;
  final List<Map<String, dynamic>> entries;
  final bool? lastSaveCommitted;
  final bool readOnly;
}

typedef _Reference = ({String name, String kind, bool archived});

enum _Form { account, category, entry }

const _accountKinds = {
  'cash': '现金',
  'bank': '银行账户',
  'wallet': '电子钱包',
  'credit_card': '信用卡',
};
const _directions = {'expense': '支出', 'income': '收入'};

String _yuan(String minor) {
  final negative = minor.startsWith('-');
  final digits = (negative ? minor.substring(1) : minor).padLeft(3, '0');
  final split = digits.length - 2;
  return '${negative ? '-' : ''}¥${digits.substring(0, split)}.${digits.substring(split)}';
}

String _newId() {
  final random = Random.secure();
  return List.generate(
    16,
    (_) => random.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
}

class LocalBookPanel extends StatefulWidget {
  const LocalBookPanel({
    super.key,
    required this.book,
    required this.unlock,
    required this.credential,
    required this.busy,
    required this.message,
    required this.clearMessage,
    required this.save,
  });

  final LocalBook book;
  final LedgerUnlock unlock;
  final TextEditingController credential;
  final bool busy;
  final String? message;
  final VoidCallback clearMessage;
  final Future<bool> Function(Map<String, Object?>) save;

  @override
  State<LocalBookPanel> createState() => _LocalBookPanelState();
}

class _LocalBookPanelState extends State<LocalBookPanel> {
  final _name = TextEditingController();
  final _amount = TextEditingController();
  final _date = TextEditingController();
  _Form? _form;
  String _accountKind = 'cash';
  String _direction = 'expense';
  String? _account;
  String? _category;
  String? _referenceId;
  Map<String, String>? _writeIdentity;
  bool _confirmDiscard = false;

  Map<String, String> get _accounts => {
    for (final MapEntry(:key, :value) in widget.book._accounts.entries)
      if (!value.archived) key: value.name,
  };

  Map<String, String> get _categories => {
    for (final MapEntry(:key, :value) in widget.book._categories.entries)
      if (!value.archived && value.kind == _direction) key: value.name,
  };

  void _open(_Form form) {
    widget.clearMessage();
    setState(() {
      _form = form;
      _name.clear();
      _amount.text = form == _Form.account ? '0' : '';
      _date.text = DateTime.now().toIso8601String().substring(0, 10);
      _accountKind = 'cash';
      _direction = 'expense';
      _account = _accounts.keys.firstOrNull;
      _category = null;
      _referenceId = null;
      _writeIdentity = null;
      _confirmDiscard = false;
    });
  }

  void _askDiscard() {
    if (widget.busy) return;
    FocusManager.instance.primaryFocus?.unfocus();
    widget.credential.clear();
    setState(() => _confirmDiscard = true);
  }

  Map<String, String> _identity() => _writeIdentity ??= {
    'operationId': _newId(),
    'eventId': _newId(),
    'revisionId': _newId(),
  };

  Future<void> _save(_Form form) async {
    final Map<String, Object?> command = switch (form) {
      _Form.account => {
        'kind': 'account',
        'id': _referenceId ??= _newId(),
        'accountKind': _accountKind,
        'name': _name.text,
        'amountYuan': _amount.text,
        'businessDate': _date.text,
        'identity': _identity(),
      },
      _Form.category => {
        'kind': 'category',
        'id': _referenceId ??= _newId(),
        'categoryKind': _direction,
        'name': _name.text,
      },
      _Form.entry => {
        'kind': 'entry',
        'direction': _direction,
        'amountYuan': _amount.text,
        'businessDate': _date.text,
        'accountId': _account,
        'categoryId': _category,
        'identity': _identity(),
      },
    };
    if (await widget.save(command) && mounted) {
      setState(() => _form = null);
    }
  }

  @override
  void dispose() {
    _name.dispose();
    _amount.dispose();
    _date.dispose();
    super.dispose();
  }

  Widget _choice(
    String label,
    Map<String, String> options,
    String? selected,
    ValueChanged<String?> change, {
    Key? key,
  }) => DropdownButtonFormField<String>(
    key: key,
    initialValue: selected,
    isExpanded: true,
    decoration: InputDecoration(labelText: label),
    items: [
      for (final MapEntry(:key, :value) in options.entries)
        DropdownMenuItem(
          value: key,
          child: Text(value, maxLines: 1, overflow: TextOverflow.ellipsis),
        ),
    ],
    onChanged: widget.busy ? null : change,
  );

  Widget _text(
    TextEditingController controller,
    String label,
    int limit, {
    TextInputType? keyboard,
  }) => TextField(
    controller: controller,
    enabled: !widget.busy,
    maxLength: limit,
    autocorrect: false,
    keyboardType: keyboard,
    decoration: InputDecoration(labelText: label, counterText: ''),
  );

  List<Widget> _fields(_Form form) {
    final entry = form == _Form.entry;
    final account = form == _Form.account;
    final hasReferences = !entry || (_account != null && _category != null);
    final noCategories = _categories.isEmpty;
    return [
      Text(switch (form) {
        _Form.account => '新建账户',
        _Form.category => '新建分类',
        _Form.entry => '记一笔',
      }),
      if (!entry) _text(_name, account ? '账户名称' : '分类名称', 64),
      if (account)
        _choice(
          '账户类型',
          _accountKinds,
          _accountKind,
          (value) => setState(() => _accountKind = value!),
        )
      else
        _choice('收支类型', _directions, _direction, (value) {
          setState(() {
            _direction = value!;
            _category = null;
          });
        }),
      if (entry) ...[
        _choice(
          '资金账户',
          _accounts,
          _account,
          (value) => setState(() => _account = value),
        ),
        _choice(
          '分类',
          _categories,
          _category,
          (value) => setState(() => _category = value),
          key: ValueKey(_direction),
        ),
        if (noCategories) Text('请先新建${_directions[_direction]}分类。'),
      ],
      if (form != _Form.category) ...[
        _text(
          _amount,
          account ? '期初金额（CNY）' : '金额（CNY）',
          24,
          keyboard: TextInputType.numberWithOptions(
            decimal: true,
            signed: account,
          ),
        ),
        _text(_date, account ? '生效日期（YYYY-MM-DD）' : '业务日期（YYYY-MM-DD）', 10),
      ],
      TextField(
        controller: widget.credential,
        enabled: !widget.busy && hasReferences,
        onChanged: (_) => setState(() {}),
        obscureText: true,
        enableSuggestions: false,
        autocorrect: false,
        enableIMEPersonalizedLearning: false,
        decoration: InputDecoration(
          labelText: widget.unlock == LedgerUnlock.recovery
              ? '保存用恢复材料'
              : '保存用本地口令',
        ),
      ),
      FilledButton(
        onPressed:
            widget.busy || !hasReferences || widget.credential.text.isEmpty
            ? null
            : () => _save(form),
        child: const Text('保存到本机'),
      ),
      if (widget.message case final message?)
        Semantics(liveRegion: true, child: Text(message)),
      TextButton(
        onPressed: widget.busy ? null : _askDiscard,
        child: const Text('取消录入'),
      ),
    ];
  }

  List<Widget> _overview(LocalBook book) => [
    if (widget.message case final message?)
      Semantics(liveRegion: true, child: Text(message)),
    if (book.readOnly)
      const Text('固定验收账簿仅供查看')
    else
      Wrap(
        spacing: 8,
        runSpacing: 4,
        children: [
          OutlinedButton(
            onPressed: widget.busy ? null : () => _open(_Form.account),
            child: const Text('新建账户'),
          ),
          OutlinedButton(
            onPressed: widget.busy ? null : () => _open(_Form.category),
            child: const Text('新建分类'),
          ),
          FilledButton(
            onPressed: widget.busy || _accounts.isEmpty
                ? null
                : () => _open(_Form.entry),
            child: const Text('记一笔'),
          ),
        ],
      ),
    Text('账户余额', style: Theme.of(context).textTheme.titleMedium),
    if (book._accounts.isEmpty) const Text('请先建立账户并确认期初金额。'),
    for (final MapEntry(:key, :value) in book._accounts.entries)
      Text(
        '${value.name} · ${_accountKinds[value.kind]} · ${_yuan(book.balances[key]!)}',
      ),
    Text('分类', style: Theme.of(context).textTheme.titleMedium),
    if (book._categories.isEmpty) const Text('还没有分类。'),
    for (final category in book._categories.values)
      Text('${category.name} · ${_directions[category.kind]}'),
    Text('最近收支（最多 20 笔）', style: Theme.of(context).textTheme.titleMedium),
    if (book.entries.isEmpty) const Text('还没有收支记录。'),
    for (final entry in book.entries)
      Text(
        "${entry['businessDate']} · ${_directions[entry['kind']]} · ${_yuan(entry['amountMinor'] as String)}\n"
        "${book._accounts[entry['accountId']]!.name} · ${book._categories[entry['categoryId']]!.name}",
      ),
  ];

  @override
  Widget build(BuildContext context) {
    final form = _form;
    final children = form == null
        ? _overview(widget.book)
        : _confirmDiscard
        ? [
            const Text('丢弃未保存的输入？'),
            TextButton(
              onPressed: () => setState(() => _confirmDiscard = false),
              child: const Text('继续编辑'),
            ),
            TextButton(
              onPressed: () => setState(() => _form = null),
              child: const Text('丢弃输入'),
            ),
          ]
        : _fields(form);
    return PopScope(
      canPop: form == null,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop && form != null) _askDiscard();
      },
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final child in children)
            Padding(padding: const EdgeInsets.only(top: 12), child: child),
        ],
      ),
    );
  }
}
