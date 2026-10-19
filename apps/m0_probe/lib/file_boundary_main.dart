// 专用合成入口；普通 main.dart 不调用，也不接受外部命令或秘密材料。
import 'package:flutter/material.dart';
import 'src/file_boundary.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final status = ValueNotifier<String>('正在复验 App 私有文件边界');
  runApp(
    MaterialApp(
      home: Scaffold(
        body: SafeArea(
          child: Center(
            child: ValueListenableBuilder<String>(
              valueListenable: status,
              builder: (context, value, child) =>
                  Text(value, textAlign: TextAlign.center),
            ),
          ),
        ),
      ),
    ),
  );
  const session = String.fromEnvironment('M0_APP_BOUNDARY_RUN');
  final report = await runAppFileBoundary(session);
  await publishAppBoundaryReport(report);
  status.value = report['passed'] == true
      ? 'App 路径与文件观测通过\n双桥接 × 两种临时存储请求\n合成账库已精确清理\n硬件文件保护仍需真机验证'
      : 'App 文件边界复验未通过\n请查看本次合成诊断报告';
}
