# RadishLedger 移动核心实验

使用合成共同材料，通过薄 C ABI 与 flutter_rust_bridge 调用同一 Rust / SQLCipher 核心。不能接收真实账单，不能作为用户账簿。

Android 内部账簿通过 `--dart-define=RADISH_INTERNAL_LEDGER=true` 进入，只使用薄 C ABI / Dart FFI。创建并确认恢复材料后，可以建立账户与分类，录入普通收支并重新打开核对。[内部账簿说明](../../docs/development/mobile-core-prototype.md#8-android-内部账簿的创建与重开) · [宿主与设备复现](../../scripts/README.md#m0-android-内部账簿创建与重开)

契约、锁定工具链、构建 / 运行入口、许可与清理见[正式实验说明](../../docs/development/mobile-core-prototype.md)。实际结果与阶段门槛见[当前状态](../../docs/status/current.md)，native 许可通过 pubspec asset 随包保留。
