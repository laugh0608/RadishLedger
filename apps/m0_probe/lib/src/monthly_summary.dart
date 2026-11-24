import 'bootstrap.dart';

class MonthlySummary {
  const MonthlySummary._({
    required this.month,
    required this.income,
    required this.expense,
    required this.refund,
    required this.netExpense,
    required this.surplus,
    required this.categories,
  });

  final String month;
  final String income;
  final String expense;
  final String refund;
  final String netExpense;
  final String surplus;
  final List<({String name, String minor})> categories;

  bool get hasActivity => income != '0' || expense != '0' || refund != '0';

  factory MonthlySummary.fromReport(
    Map<String, dynamic> report,
    String month,
    int sequence,
  ) {
    const invalid = BootstrapFailure('invalid_response');
    final data = report['monthSummary'];
    if (data is! Map<String, dynamic> ||
        data['month'] != month ||
        data['sequence'] != sequence) {
      throw invalid;
    }
    String amount(Object? value) {
      if (value is! String || int.tryParse(value)?.toString() != value) {
        throw invalid;
      }
      return value;
    }

    final names = data['categoryNames'];
    final amounts = data['expenseCategoriesMinor'];
    if (names is! Map<String, dynamic> ||
        amounts is! Map<String, dynamic> ||
        names.length != amounts.length) {
      throw invalid;
    }
    final rows = <({String name, String minor})>[];
    for (final entry in amounts.entries) {
      final name = names[entry.key];
      if (name is! String) throw invalid;
      rows.add((name: name, minor: amount(entry.value)));
    }
    rows.sort((a, b) {
      final byTotal = int.parse(b.minor).compareTo(int.parse(a.minor));
      return byTotal != 0 ? byTotal : a.name.compareTo(b.name);
    });
    return MonthlySummary._(
      month: month,
      income: amount(data['incomeMinor']),
      expense: amount(data['grossExpenseMinor']),
      refund: amount(data['refundMinor']),
      netExpense: amount(data['netExpenseMinor']),
      surplus: amount(data['surplusMinor']),
      categories: rows,
    );
  }
}
