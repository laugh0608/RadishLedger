# RadishLedger 移动核心实验

使用合成共同材料，通过薄 C ABI 与 flutter_rust_bridge 调用同一 Rust / SQLCipher 核心。不能接收真实账单，不能作为用户账簿。

Android 内部账簿通过 `--dart-define=RADISH_INTERNAL_LEDGER=true` 进入，使用薄 C ABI / Dart FFI。创建账簿并确认恢复材料后，可以建立账户与分类，录入、编辑和删除普通收支，在完整流水和月汇总核对结果。账簿概览可将独立加密归档保存到下载目录。创建页或解锁页可选择归档，在独立空暂存核对账务，当前活动账簿保持原状。[内部账簿说明](../../docs/development/mobile-core-prototype.md#8-android-内部账簿的创建与重开) · [宿主与设备复现](../../scripts/README.md#m0-android-内部账簿创建与重开)

契约、锁定工具链、构建 / 运行入口、许可与清理见[正式实验说明](../../docs/development/mobile-core-prototype.md)。实际结果与阶段门槛见[当前状态](../../docs/status/current.md)，native 许可通过 pubspec asset 随包保留。
