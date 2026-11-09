import 'package:flutter/material.dart';
import 'bootstrap_lock_review.dart';

class BootstrapLockReviewControls extends StatelessWidget {
  const BootstrapLockReviewControls({
    super.key,
    required this.review,
    required this.enabled,
    required this.prepare,
    required this.observe,
    required this.verify,
  });
  final BootstrapLockReview? review;
  final bool enabled;
  final void Function(String unlock) prepare;
  final VoidCallback observe;
  final void Function(String unlock) verify;

  @override
  Widget build(BuildContext context) {
    final current = review;
    final canPrepare =
        enabled &&
        (current == null || current.state == LockReviewState.matched);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('锁定前后同库复核', style: TextStyle(fontWeight: FontWeight.w600)),
        const Text(
          '先解锁建立基线，再启动 45 秒文件观测并手动锁屏。返回后重新提供解锁材料。仅本进程保留非秘密基线；重开页面后不能比较上次结果。',
        ),
        Text(
          current == null
              ? '当前没有复核基线，请先完成合成账务，再建立基线。'
              : switch (current.state) {
                  LockReviewState.baselineSaved => '基线已保存，输入已清除；可以启动锁屏观测。',
                  LockReviewState.awaitingObservation =>
                    '正在观测，请锁屏后在 45 秒内返回；超时未执行会单独记录。',
                  LockReviewState.matched =>
                    '同一账库复核一致：完整摘要、身份、8 个修订、7 个未同步操作、2 个候选、1 个删除。',
                  LockReviewState.mismatch => '复核不一致，保留原库和基线；不会自动修复或重写。',
                  LockReviewState.unlockFailed => '重新解锁未完成，原库和基线保留，可重试。',
                  LockReviewState.interrupted => '复核等待已停止，请重新输入材料复核。',
                  _ => '观测窗口已结束，请重新解锁并复核同一账库。',
                },
          key: const ValueKey('m0c-review-state'),
        ),
        if (current != null)
          Text(
            '文件观测：${switch (current.fileOutcome) {
              LockFileOutcome.notStarted => '尚未开始',
              LockFileOutcome.observing => '进行中',
              LockFileOutcome.observed => '已执行锁定与恢复阶段，拒绝结果见诊断',
              LockFileOutcome.cancelled => '已取消',
              LockFileOutcome.lockedIoNotExecuted => '超时，未执行锁定阶段 IO',
              LockFileOutcome.restorationNotObserved => '未在窗口内观测到恢复',
              LockFileOutcome.unsupportedSimulator => '模拟器不支持真实锁屏保护观测',
              LockFileOutcome.unsupportedPlatform => '当前平台未接入锁屏文件观测',
              LockFileOutcome.baselineUnavailable => '系统保护数据暂不可用',
              LockFileOutcome.baselineFailed => '文件访问基线失败',
              _ => '观测失败，保留现场',
            }}。账务一致不代表锁屏保护验收通过。',
            key: const ValueKey('m0c-review-files'),
          ),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            for (final (unlock, label) in [
              ('password', '口令'),
              ('recovery', '恢复材料'),
              ('system', '系统认证'),
            ])
              OutlinedButton(
                key: ValueKey('m0c-review-prepare-$unlock'),
                onPressed: canPrepare ? () => prepare(unlock) : null,
                child: Text('$label建立基线'),
              ),
            OutlinedButton(
              key: const ValueKey('m0c-review-observe'),
              onPressed:
                  enabled && current?.fileOutcome == LockFileOutcome.notStarted
                  ? observe
                  : null,
              child: const Text('启动锁屏观测'),
            ),
            for (final (unlock, label) in [
              ('password', '口令'),
              ('recovery', '恢复材料'),
              ('system', '系统认证'),
            ])
              OutlinedButton(
                key: ValueKey('m0c-review-verify-$unlock'),
                onPressed: enabled && current?.canVerify == true
                    ? () => verify(unlock)
                    : null,
                child: Text('$label重新解锁并复核'),
              ),
          ],
        ),
        const SizedBox(height: 16),
      ],
    );
  }
}
