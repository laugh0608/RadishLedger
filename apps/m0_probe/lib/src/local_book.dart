import 'dart:math';

import 'package:flutter/material.dart';

import 'bootstrap.dart';
import 'internal_ledger.dart';
import 'monthly_summary.dart';

class LocalBook {
  LocalBook._(
    this._accounts,
    this._categories,
    this.balances,
    this.firstPage,
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
      EntryPage.fromJson(book, index: 0),
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
  final EntryPage firstPage;

  EntryPage readPage(Map<String, dynamic> report, int index) {
    final page = EntryPage.fromJson(report['entryPage'], index: index);
    if (page.sequence != firstPage.sequence ||
        page._entries.any(
          (row) =>
              !_accounts.containsKey(row.accountId) ||
              !_categories.containsKey(row.categoryId),
        )) {
      throw const BootstrapFailure('invalid_response');
    }
    return page;
  }

  final bool? lastSaveCommitted;
  final bool readOnly;
}

class EntryPage {
  EntryPage._(this.index, this.sequence, this.hasMore, this._entries);

  factory EntryPage.fromJson(dynamic value, {required int index}) {
    if (value is! Map<String, dynamic> ||
        value['pageIndex'] != index ||
        value['sequence'] is! int ||
        value['hasMore'] is! bool ||
        value['recentEntries'] is! List) {
      throw const BootstrapFailure('invalid_response');
    }
    return EntryPage._(
      index,
      value['sequence'] as int,
      value['hasMore'] as bool,
      [
        for (final row
            in (value['recentEntries'] as List).cast<Map<String, dynamic>>())
          _BookEntry.fromJson(row),
      ],
    );
  }

  final int index;
  final int sequence;
  final bool hasMore;
  final List<_BookEntry> _entries;
}

typedef _Reference = ({String name, String kind, bool archived});

class _BookEntry {
  _BookEntry.fromJson(Map<String, dynamic> row)
    : id = row['id'] as String,
      revisionId = row['revisionId'] as String,
      kind = row['kind'] as String,
      businessDate = row['businessDate'] as String,
      accountId = row['accountId'] as String,
      categoryId = row['categoryId'] as String,
      amountMinor = row['amountMinor'] as String;

  final String id;
  final String revisionId;
  final String kind;
  final String businessDate;
  final String accountId;
  final String categoryId;
  final String amountMinor;

  String describe(LocalBook book) =>
      '$businessDate · ${_directions[kind]} · ${_yuan(amountMinor)}\n'
      '${book._accounts[accountId]!.name} · ${book._categories[categoryId]!.name}';
}

enum _MonthView { choose, report }

enum _Form { account, category, entry, revise, delete }

const _accountKinds = {
  'cash': '现金',
  'bank': '银行账户',
  'wallet': '电子钱包',
  'credit_card': '信用卡',
};
const _directions = {'expense': '支出', 'income': '收入'};

String _yuan(String minor, {bool currencySign = true}) {
  final negative = minor.startsWith('-');
  final digits = (negative ? minor.substring(1) : minor).padLeft(3, '0');
  final split = digits.length - 2;
  return '${negative ? '-' : ''}${currencySign ? '¥' : ''}${digits.substring(0, split)}.${digits.substring(split)}';
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
    required this.readPage,
    required this.readMonth,
  });

  final LocalBook book;
  final LedgerUnlock unlock;
  final TextEditingController credential;
  final bool busy;
  final String? message;
  final VoidCallback clearMessage;
  final Future<EntryPage?> Function(int) readPage;
  final Future<MonthlySummary?> Function(String) readMonth;
  final Future<bool> Function(Map<String, Object?>) save;

  @override
  State<LocalBookPanel> createState() => _LocalBookPanelState();
}

class _LocalBookPanelState extends State<LocalBookPanel> {
  final _name = TextEditingController();
  final _amount = TextEditingController();
  final _date = TextEditingController();
  final _month = TextEditingController();
  _MonthView? _monthView;
  MonthlySummary? _summary;
  _Form? _form;
  _BookEntry? _original;
  String _accountKind = 'cash';
  String _direction = 'expense';
  String? _account;
  String? _category;
  String? _referenceId;
  Map<String, String>? _writeIdentity;
  bool _confirmDiscard = false;
  EntryPage? _history;
  int? _requestedPage;

  void _chooseMonth(String month) {
    widget.clearMessage();
    widget.credential.clear();
    _month.text = month;
    setState(() => _monthView = _MonthView.choose);
  }

  void _openMonths() {
    if (_summary != null) {
      widget.clearMessage();
      setState(() => _monthView = _MonthView.report);
      return;
    }
    final latest = widget.book.firstPage._entries.firstOrNull?.businessDate;
    _chooseMonth((latest ?? DateTime.now().toIso8601String()).substring(0, 7));
  }

  void _leaveMonth() {
    widget.clearMessage();
    widget.credential.clear();
    setState(
      () => _monthView = _monthView == _MonthView.choose && _summary != null
          ? _MonthView.report
          : null,
    );
  }

  Future<void> _readMonth() async {
    final report = await widget.readMonth(_month.text);
    if (mounted && report != null) {
      setState(() {
        _summary = report;
        _monthView = _MonthView.report;
      });
    }
  }

  void _shiftMonth(String month, int offset) {
    final date = DateTime.utc(
      int.parse(month.substring(0, 4)),
      int.parse(month.substring(5)) + offset,
    );
    _chooseMonth(date.toIso8601String().substring(0, 7));
  }

  void _showPage(EntryPage? page) {
    widget.clearMessage();
    widget.credential.clear();
    setState(() {
      _history = page;
      _requestedPage = null;
    });
  }

  void _navigate(int index) {
    if (index == 0) {
      _showPage(widget.book.firstPage);
      return;
    }
    widget.clearMessage();
    setState(() => _requestedPage = index);
  }

  Future<void> _readPage(int index) async {
    final page = await widget.readPage(index);
    if (mounted && page != null) _showPage(page);
  }

  Map<String, String> get _accounts => {
    for (final MapEntry(:key, :value) in widget.book._accounts.entries)
      if (!value.archived) key: value.name,
  };

  Map<String, String> get _categories => {
    for (final MapEntry(:key, :value) in widget.book._categories.entries)
      if (!value.archived && value.kind == _direction) key: value.name,
  };

  void _open(_Form form, {_BookEntry? original}) {
    widget.clearMessage();
    setState(() {
      _form = form;
      _original = original;
      _name.clear();
      _amount.text = original != null
          ? _yuan(original.amountMinor, currencySign: false)
          : (form == _Form.account ? '0' : '');
      _date.text =
          original?.businessDate ??
          DateTime.now().toIso8601String().substring(0, 10);
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
    if (_form == _Form.delete) {
      widget.clearMessage();
      setState(() => _form = null);
      return;
    }
    setState(() => _confirmDiscard = true);
  }

  Map<String, String> _identity() => _writeIdentity ??= {
    'operationId': _newId(),
    'eventId': _original?.id ?? _newId(),
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
      _Form.revise => {
        'kind': 'reviseEntry',
        'previousRevisionId': _original!.revisionId,
        'amountYuan': _amount.text,
        'businessDate': _date.text,
        'identity': _identity(),
      },
      _Form.delete => {
        'kind': 'deleteEntry',
        'previousRevisionId': _original!.revisionId,
        'identity': _identity(),
      },
    };
    if (await widget.save(command) && mounted) {
      setState(() {
        _form = null;
        _history = null;
        _summary = null;
      });
    }
  }

  @override
  void dispose() {
    _name.dispose();
    _amount.dispose();
    _date.dispose();
    _month.dispose();
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

  Widget _credentialField(String action, {bool enabled = true}) => TextField(
    controller: widget.credential,
    enabled: !widget.busy && enabled,
    onChanged: (_) => setState(() {}),
    obscureText: true,
    enableSuggestions: false,
    autocorrect: false,
    enableIMEPersonalizedLearning: false,
    decoration: InputDecoration(
      labelText:
          '$action用${widget.unlock == LedgerUnlock.recovery ? '恢复材料' : '本地口令'}',
    ),
  );

  List<Widget> _fields(_Form form) {
    final entry = form == _Form.entry;
    final account = form == _Form.account;
    final deleting = form == _Form.delete;
    final hasReferences = !entry || (_account != null && _category != null);
    final noCategories = _categories.isEmpty;
    return [
      Text(switch (form) {
        _Form.account => '新建账户',
        _Form.category => '新建分类',
        _Form.entry => '记一笔',
        _Form.revise => '编辑收支',
        _Form.delete => '删除收支',
      }),
      if (_original case final original?) ...[
        Text('原记录\n${original.describe(widget.book)}'),
        if (deleting)
          Text(
            '这笔${_directions[original.kind]}将从流水和当月汇总中移除，'
            '${widget.book._accounts[original.accountId]!.name}余额将'
            '${original.kind == 'expense' ? '增加' : '减少'}${_yuan(original.amountMinor)}。',
          ),
      ],
      if (account || form == _Form.category)
        _text(_name, account ? '账户名称' : '分类名称', 64),
      if (account)
        _choice(
          '账户类型',
          _accountKinds,
          _accountKind,
          (value) => setState(() => _accountKind = value!),
        )
      else if (entry || form == _Form.category)
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
      if (!deleting && form != _Form.category) ...[
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
      _credentialField(deleting ? '删除' : '保存', enabled: hasReferences),
      FilledButton(
        onPressed:
            widget.busy || !hasReferences || widget.credential.text.isEmpty
            ? null
            : () => _save(form),
        child: Text(deleting ? '确认删除' : '保存到本机'),
      ),
      if (widget.message case final message?)
        Semantics(liveRegion: true, child: Text(message)),
      TextButton(
        onPressed: widget.busy ? null : _askDiscard,
        child: Text(deleting ? '取消删除' : '取消录入'),
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
    OutlinedButton(
      onPressed: widget.busy ? null : _openMonths,
      child: const Text('查看月汇总'),
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
    TextButton(
      onPressed: widget.busy ? null : () => _showPage(book.firstPage),
      child: const Text('查看完整流水'),
    ),
    ..._entryRows(book, book.firstPage),
  ];

  List<Widget> _monthlyView() {
    if (_monthView == _MonthView.choose) {
      return [
        Text('查询月汇总', style: Theme.of(context).textTheme.titleMedium),
        _text(_month, '业务月份（YYYY-MM）', 7),
        _credentialField('查询'),
        FilledButton(
          onPressed: widget.busy || widget.credential.text.isEmpty
              ? null
              : _readMonth,
          child: const Text('读取月汇总'),
        ),
        if (widget.message case final message?)
          Semantics(liveRegion: true, child: Text(message)),
        TextButton(
          onPressed: widget.busy ? null : _leaveMonth,
          child: const Text('取消查询'),
        ),
      ];
    }
    final summary = _summary!;
    return [
      Text(
        '${summary.month} 月汇总',
        style: Theme.of(context).textTheme.titleMedium,
      ),
      Wrap(
        children: [
          TextButton(
            onPressed: widget.busy || summary.month == '0001-01'
                ? null
                : () => _shiftMonth(summary.month, -1),
            child: const Text('上个月'),
          ),
          TextButton(
            onPressed: widget.busy ? null : () => _chooseMonth(summary.month),
            child: const Text('选择月份'),
          ),
          TextButton(
            onPressed: widget.busy || summary.month == '9999-12'
                ? null
                : () => _shiftMonth(summary.month, 1),
            child: const Text('下个月'),
          ),
        ],
      ),
      if (!summary.hasActivity) const Text('这个月没有收支或退款。'),
      Text('收入 ${_yuan(summary.income)}'),
      Text('净支出 ${_yuan(summary.netExpense)}'),
      Text('结余 ${_yuan(summary.surplus)}'),
      Text('毛支出 ${_yuan(summary.expense)} · 退款 ${_yuan(summary.refund)}'),
      Text('支出分类净额', style: Theme.of(context).textTheme.titleSmall),
      if (summary.categories.isEmpty) const Text('还没有支出分类。'),
      for (final row in summary.categories)
        Text('${row.name} · ${_yuan(row.minor)}'),
      TextButton(
        onPressed: widget.busy ? null : _leaveMonth,
        child: const Text('返回账簿'),
      ),
    ];
  }

  List<Widget> _historyView(LocalBook book, EntryPage page) => [
    Text(
      '完整流水 · 第 ${page.index + 1} 页',
      style: Theme.of(context).textTheme.titleMedium,
    ),
    if (_requestedPage case final next?) ...[
      _credentialField('翻页'),
      FilledButton(
        onPressed: widget.busy || widget.credential.text.isEmpty
            ? null
            : () => _readPage(next),
        child: Text('读取第 ${next + 1} 页'),
      ),
      TextButton(
        onPressed: widget.busy ? null : () => _showPage(page),
        child: const Text('取消翻页'),
      ),
    ] else ...[
      Wrap(
        children: [
          if (page.index > 0)
            TextButton(
              onPressed: widget.busy ? null : () => _navigate(page.index - 1),
              child: const Text('上一页'),
            ),
          if (page.hasMore)
            TextButton(
              onPressed: widget.busy ? null : () => _navigate(page.index + 1),
              child: const Text('下一页'),
            ),
          TextButton(
            onPressed: widget.busy ? null : () => _showPage(null),
            child: const Text('返回账簿'),
          ),
        ],
      ),
      if (!page.hasMore && page._entries.isNotEmpty) const Text('已到最后一页'),
      ..._entryRows(book, page),
      if (page.hasMore)
        OutlinedButton(
          onPressed: widget.busy ? null : () => _navigate(page.index + 1),
          child: const Text('继续查看下一页'),
        ),
    ],
    if (widget.message case final message?)
      Semantics(liveRegion: true, child: Text(message)),
  ];

  List<Widget> _entryRows(LocalBook book, EntryPage page) => [
    if (page._entries.isEmpty) const Text('还没有收支记录。'),
    for (final entry in page._entries) ...[
      Text(entry.describe(book)),
      if (!book.readOnly)
        Wrap(
          children: [
            TextButton.icon(
              icon: const Icon(Icons.edit_outlined),
              label: Text('编辑这笔${_directions[entry.kind]}'),
              onPressed: widget.busy
                  ? null
                  : () => _open(_Form.revise, original: entry),
            ),
            TextButton.icon(
              icon: const Icon(Icons.delete_outline),
              label: Text('删除这笔${_directions[entry.kind]}'),
              onPressed: widget.busy
                  ? null
                  : () => _open(_Form.delete, original: entry),
            ),
          ],
        ),
    ],
  ];

  @override
  Widget build(BuildContext context) {
    final form = _form;
    final history = _history;
    final children = _monthView != null
        ? _monthlyView()
        : form == null
        ? history == null
              ? _overview(widget.book)
              : _historyView(widget.book, history)
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
      canPop: form == null && history == null && _monthView == null,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop || widget.busy) return;
        if (_monthView != null) {
          _leaveMonth();
        } else if (form != null) {
          _askDiscard();
        } else {
          _showPage(_requestedPage == null ? null : history);
        }
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
