import 'package:flutter/material.dart';
import 'src/probe.dart';

typedef ProbeRun = Future<ProbeReport> Function(BridgeCandidate candidate);

class ProbeApp extends StatelessWidget {
  const ProbeApp({super.key, this.run = runProbe});
  final ProbeRun run;
  @override
  Widget build(BuildContext context) => MaterialApp(
    title: '萝卜账簿 · 核心验证',
    theme: _theme(false),
    darkTheme: _theme(true),
    home: _ProbePage(run: run),
  );
  ThemeData _theme(bool dark) {
    final ink = Color(dark ? 0xffede5d8 : 0xff2f2a25);
    final action = Color(dark ? 0xff7fa0bd : 0xff435c74);
    final background = Color(dark ? 0xff1a1713 : 0xfff7f4ee);
    final surface = Color(dark ? 0xff23201a : 0xfffffdf8);
    return ThemeData(
      brightness: dark ? Brightness.dark : Brightness.light,
      scaffoldBackgroundColor: background,
      colorScheme:
          ColorScheme.fromSeed(
            seedColor: action,
            brightness: dark ? Brightness.dark : Brightness.light,
          ).copyWith(
            primary: action,
            onPrimary: dark ? background : surface,
            surface: surface,
            onSurface: ink,
            surfaceTint: Colors.transparent,
          ),
      appBarTheme: AppBarTheme(
        backgroundColor: background,
        foregroundColor: ink,
        surfaceTintColor: Colors.transparent,
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          minimumSize: const Size(48, 48),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
          ),
        ),
      ),
      inputDecorationTheme: const InputDecorationTheme(
        border: OutlineInputBorder(),
      ),
    );
  }
}

class _ProbePage extends StatefulWidget {
  const _ProbePage({required this.run});
  final ProbeRun run;
  @override
  State<_ProbePage> createState() => _ProbePageState();
}

class _ProbePageState extends State<_ProbePage> {
  BridgeCandidate _candidate = BridgeCandidate.thin;
  bool _busy = false;
  bool _cancelled = false;
  ProbeReport? _report;
  String? _error;
  Future<void> _start() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _cancelled = false;
      _report = null;
      _error = null;
    });
    try {
      final report = await widget.run(_candidate);
      if (mounted && !_cancelled) {
        setState(() {
          _report = report;
        });
      }
    } catch (error) {
      if (mounted && !_cancelled) {
        setState(() {
          _error = error.toString();
        });
      }
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(title: const Text('萝卜账簿 · 核心验证')),
    body: SafeArea(
      child: ListView(
        padding: const EdgeInsets.all(20),
        children: [
          const Text(
            '本机账务闭环',
            style: TextStyle(fontSize: 26, fontWeight: FontWeight.w600),
          ),
          const SizedBox(height: 12),
          const Text('使用共同合成材料，验证 Rust 账务、加密事务、重复提交和重新打开。实验目录与密钥仅用于本次验证。'),
          const SizedBox(height: 24),
          DropdownButtonFormField<BridgeCandidate>(
            key: const ValueKey('bridge-candidate'),
            initialValue: _candidate,
            decoration: const InputDecoration(labelText: '接入候选'),
            items: const [
              DropdownMenuItem(
                value: BridgeCandidate.thin,
                child: Text('薄 C ABI / Dart FFI'),
              ),
              DropdownMenuItem(
                value: BridgeCandidate.frb,
                child: Text('flutter_rust_bridge'),
              ),
            ],
            onChanged: _busy
                ? null
                : (value) {
                    if (value != null) {
                      setState(() {
                        _candidate = value;
                      });
                    }
                  },
          ),
          const SizedBox(height: 16),
          FilledButton(
            key: const ValueKey('run-probe'),
            onPressed: _busy ? null : _start,
            child: Text(_busy ? '正在验证本机核心' : '运行合成验证'),
          ),
          if (_busy) ...[
            const SizedBox(height: 16),
            const LinearProgressIndicator(),
            TextButton(
              key: const ValueKey('stop-waiting'),
              onPressed: _cancelled
                  ? null
                  : () {
                      setState(() {
                        _cancelled = true;
                      });
                    },
              child: const Text('停止等待'),
            ),
          ],
          if (_cancelled)
            Padding(
              padding: const EdgeInsets.only(top: 16),
              child: Text(
                _busy ? '已停止等待显示；核心仍在完成当前实验并清理临时目录。' : '当前实验已结束，可以重新运行。',
              ),
            ),
          if (_error != null)
            Padding(
              padding: const EdgeInsets.only(top: 20),
              child: SelectableText(
                '验证失败\n$_error',
                key: const ValueKey('probe-error'),
              ),
            ),
          if (_report case final report?) ...[
            const SizedBox(height: 24),
            const Text(
              '本机验证通过',
              key: ValueKey('probe-success'),
              style: TextStyle(fontSize: 22, fontWeight: FontWeight.w600),
            ),
            const SizedBox(height: 12),
            Text('${report.caseCount} 个案例 · ${report.checkpointCount} 个检查点'),
            Text('SQLCipher ${report.cipherVersion}'),
            Text('实际核心目标：${report.nativeTarget}'),
            const SizedBox(height: 16),
            const Text(
              '已核对事务拒绝后的完整回滚、重复提交、关闭后重开和错误密钥拒绝。该结果属于本次接入实验，系统密钥与正式账库恢复另行验证。',
            ),
          ],
          const SizedBox(height: 24),
          const Text('合成数据验证应用', style: TextStyle(fontSize: 12)),
        ],
      ),
    ),
  );
}
