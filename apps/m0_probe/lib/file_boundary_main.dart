// 专用合成入口；普通 main.dart 不调用，也不接受外部命令或秘密材料。
import 'package:flutter/material.dart';
import 'src/file_boundary.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final status = ValueNotifier<String>('正在复验 App 私有文件边界');
  final waiting = ValueNotifier<bool>(false);
  runApp(
    MaterialApp(
      home: Scaffold(
        body: SafeArea(
          child: Center(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                ValueListenableBuilder<String>(
                  valueListenable: status,
                  builder: (context, value, child) =>
                      Text(value, textAlign: TextAlign.center),
                ),
                ValueListenableBuilder<bool>(
                  valueListenable: waiting,
                  builder: (context, value, child) => value
                      ? TextButton(
                          onPressed: () async {
                            try {
                              await cancelFileProtectionObservation();
                            } catch (_) {
                              status.value = '取消未完成，请解锁后等待观测返回';
                            }
                          },
                          child: const Text('取消锁屏观测'),
                        )
                      : const SizedBox.shrink(),
                ),
              ],
            ),
          ),
        ),
      ),
    ),
  );
  const session = String.fromEnvironment('M0_APP_BOUNDARY_RUN');
  const observeIosLock = bool.fromEnvironment('M0_IOS_LOCK_OBSERVATION');
  final report = await runAppFileBoundary(
    session,
    observeIosLock: observeIosLock,
    onLockObservation: (value) {
      waiting.value = value;
      status.value = value
          ? '锁屏观测已启动（45 秒窗口）\n请按已批准操作单锁屏，再解锁返回\n暂停期间未执行 IO 将如实报告'
          : '正在完成文件观测与精确清理';
    },
  );
  await publishAppBoundaryReport(report);
  status.value = report['passed'] == true
      ? 'App 路径与文件观测通过\n双桥接 × 两种临时存储请求\n合成账库已精确清理\n硬件文件保护仍需真机验证'
      : 'App 文件边界复验未通过\n请查看本次合成诊断报告';
  if (report['lockObservation'] case final Map observation) {
    status.value +=
        '\n锁屏观测：${observation['outcome']}\n'
        '读 / 写打开拒绝：${observation['accessDeniedObserved'] ?? '未执行'}\n'
        '访问恢复：${observation['accessRestored'] ?? '未执行'}\n'
        '对照文件实际写拒绝：${observation['controlWriteDeniedObserved'] ?? '未执行'}\n'
        '对照文件写入恢复：${observation['controlWriteRestored'] ?? '未执行'}';
  }
}
