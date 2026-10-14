# 移动核心接入实验

本篇描述 `apps/m0_probe`、`crates/ledger-core` 与 `crates/ledger-mobile` 的实验契约和复现入口。目标、退出条件与未覆盖的设备边界见[验证矩阵](mobile-core-validation.md)，实际结果和下一顺位见[当前状态](../status/current.md)。实验不能接收真实账单或成为用户账簿。

## 1. 一个核心、两个调用候选

```text
Flutter 验证界面 → Dart 工作 isolate → 薄 C ABI 或 flutter_rust_bridge
                                         ↓
                               同一个 Rust probe / 领域 / Store
                                         ↓
                               隔离 SQLCipher 文件与事务 outbox
```

两个候选调用同一个 `run_probe(directory)`，输入只允许存在的绝对临时目录。Rust 在其下独占创建随机实验子目录，生成内存中的随机数据库密钥，执行[共同材料](../validation/accounting-cases.md)，核对实际结果后关闭并删除自身实验子目录。Flutter 不计算余额，不生成预期，也不传入数据库密钥。

领域层按规范整数分与 Gregorian 日期校验，转账使用一项事实表达两侧变动；余额、正负资产、月报和分类结果由有效修订派生。存储层保留修订、删除标记及冲突候选，在同一 SQLite 事务写入选中事实、outbox 和序号。重复 operation identity 只接受完全相同的意图；对账校验观察序号、观察余额和固定差额。

probe 使用真实 SQLCipher 事务：每次保存前在 outbox 插入点以测试触发器拒绝，核对事实、修订、序号和队列均回滚，再执行正常保存及重复提交。每个案例关闭后使用同一内存密钥重新打开并核对，另用随机错钥尝试打开。金额预期来自手写 JSON，集合输出在替换合成别名后排序以保持跨调用可比较。

这是固定合成材料的接入实验。账户 / 分类由实验准备动作建立；没有生产账户创建原子流程、用户开库、进程重启后的解锁、系统密钥 adapter、备份或迁移。schema `1`、C ABI `1` 和 JSON 返回结构都是实验契约，不能解释为生产格式冻结。上述共同案例的数据库密钥始终只在本次 Rust 进程内存；它不是完整 bootstrap 或独立恢复方案。跨进程故障 runner 使用另行限定的公开合成测试材料，见下文。

## 2. 薄 C ABI 契约

正式声明见 [radishledger_probe.h](../../crates/ledger-mobile/include/radishledger_probe.h)，Dart 调用见 [native.dart](../../apps/m0_probe/lib/src/native.dart)。

| 边界 | 约定 |
| --- | --- |
| 版本 | 调用前必须核对 ABI `1`，不猜测兼容或返回默认成功 |
| 输入 | UTF-8 字节与显式长度；非空、最多 4096 bytes，内存由调用方所有并在同步调用期间存活 |
| 输出 | 最多 1 MiB 的 UTF-8 JSON；返回递增 `u64` 句柄，最多同时保留 16 个结果；`0` 表示无法取得结果 |
| 读取 | 先查长度，再复制到独立调用方缓冲区；短容量或无效句柄不写入内存 |
| 释放 | Rust 句柄由 Rust 释放；Dart 缓冲区由 Dart 释放；重复释放拒绝，不把 Rust 指针交给 Dart allocator |
| 指针 | C 调用者必须提供有效可读 / 可写内存；长度检查不能证明任意外部地址有效 |
| 错误 | `{ok:false,error:{code,message,sqliteCode?}}`；存储保留 SQLite extended code，不输出 SQL、输入或密钥 |
| panic | C 入口捕获可展开 panic；内存不足 abort / 无效外部指针等进程级故障不宣称可恢复 |
| 线程 | Rust 执行发生在 Dart 工作 isolate；结果注册表互斥，账务文件与密钥按调用隔离 |
| 取消 | UI 停止等待并抑制晚返回展示，底层工作仍完成并释放资源；结束前不允许开始重叠任务 |

`library_path` 通过已加载导出函数的 `dladdr` 返回实际 code asset 路径，FRB 使用该路径加载同一库；不靠猜测 App bundle 目录或回退宿主库。

FRB runtime / generator 均为 `2.13.0`，Rust 输入只有 `crate::api`；生成入口为 [generate-mobile-bridge.py](../../scripts/generate-mobile-bridge.py)。生成代码保留上游标记，禁止手改；重新生成要求项目内已安装的同版 generator 与 `cargo-expand 1.0.118`，脚本核对版本并限制安装根，不允许生成器自动安装全局工具。

## 3. 工具链与构建候选

以下为实验锁定值和前置条件，不是产品支持承诺。已有工具链优先复用，新增构建工具与缓存放入被忽略的 `local-data/`；项目脚本不安装全局工具或更新系统证书；Gradle 可在 SDK license 已接受时自动安装缺少的 SDK 组件，执行前须核对该副作用。

| 组件 | 实验值 / 约束 |
| --- | --- |
| Flutter / Dart | Flutter `3.44.0`、revision `559ffa3f75e7402d65a8def9c28389a9b2e6fe42`；Dart `3.12.0` |
| Rust | `1.96.0`；root workspace、`rust-toolchain.toml`、`Cargo.lock`；fmt / clippy 必需 |
| Android | arm64；API `24` 构建候选、compile SDK `36`、Build-Tools `36.0.0`（AGP 默认）、NDK `28.2.13676358`；JDK `17` |
| Gradle / AGP | Gradle `9.1.0` bin 发行包及官方 SHA-256；AGP `9.0.1`，沿用 Flutter 模板的兼容开关 |
| iOS | arm64 真机与 arm64 Simulator；App deployment target `15.0`，符合本批 Xcode `27.0` SDK 的目标范围 |
| 存储 | rusqlite `0.40.2`、libsqlite3-sys `0.38.2`，内含 SQLCipher `4.14.0 community` / SQLite `3.51.3` |
| 密码底层 | Apple 使用 CommonCrypto；Android / Linux 验证宿主静态构建 OpenSSL `3.6.3`，不依赖开发机动态 libcrypto |
| Dart code assets | hooks `1.0.2`、code_assets `1.0.0`、ffi `2.2.0`；`pubspec.lock` 固定传递依赖 |

实验采用 Dart build hook 构建 Rust release code asset。构建入口 [build-mobile-core.py](../../scripts/build-mobile-core.py) 根据真实目标选择 Rust triple，传递 NDK clang / ar / ranlib 或 Apple SDK 与 deployment 参数；不支持的目标明确拒绝，缺库不能生成空资产或使用另一平台库代替。Flutter 当前传给 hook 的 iOS native 最低版本可能低于 App 的 deployment target，最终 App 安装下限仍为 `15.0`，不据此承诺较旧系统可用。

两项兼容约束留在正式来源：`libc = 0.2.189` 是 FRB 所含 backtrace 在 iOS 的兼容候选，`0.2.190` 的 dyld 声明限制已实测阻断 iOS；Rust release 暂不 strip，避免 Apple 27 SDK 下 [Rust / LLVM LINKEDIT 对齐问题](https://github.com/rust-lang/rust/issues/157750)。解除约束需重新通过宿主和移动目标的真实加载，不能只看编译成功。

模板来源、第三方许可与归属见 [THIRD_PARTY_NOTICES](../../THIRD_PARTY_NOTICES.md)。所有依赖升级、最低系统冻结和 bridge 采用结论另按实际证据评审，本篇不替代 ADR。

## 4. 复现入口

```bash
# 在仓库根，使用已准备的固定工具链；pub / Cargo 缓存不齐时会下载锁定包。
./scripts/check-core.sh
./scripts/check-mobile.sh
./scripts/check-repo.sh

# 仅在确有 API 改动且同版工具已准备时运行。
python3 -B scripts/generate-mobile-bridge.py
python3 -B scripts/collect-native-notices.py --check
```

Android 打包前在 `local-data/signing/m0-probe-debug.keystore` 准备独立实验 debug keystore，alias 与口令沿用 Android 标准 debug 契约；不得复用正式发布签名。项目配置将 debug / release 的测试签名都指向该隔离文件。JDK 不通过 `flutter config` 改写全局选择，Gradle 缓存按本次进程变量隔离。Flutter 可能优先选择 Android Studio 的 JBR，因此还需在项目专属 `local-data/gradle/gradle.properties` 设置 `org.gradle.java.home` 指向本地 JDK；launcher 可以来自 Studio，实际 Gradle daemon 必须使用固定 JDK。

```bash
# 仓库根；目录内应已存在经过官方 SHA-256 核对的本地 JDK。
LEDGER_ROOT="$PWD"
export JAVA_HOME="$LEDGER_ROOT/local-data/toolchains/jdk-17.0.20.1+1/Contents/Home"
export GRADLE_USER_HOME="$LEDGER_ROOT/local-data/gradle"
cd apps/m0_probe

flutter build apk --release --target-platform android-arm64
flutter build ios --release --no-codesign

# DEVICE_ID 为已核对的现有模拟器标识；记录机型、系统版本和运行状态。
flutter drive --driver=test_driver/integration_test.dart \
  --target=integration_test/probe_test.dart --device-id=DEVICE_ID
```

Android Build-Tools 使用锁定 AGP 9.0.1 的默认 36.0.0，App 不单独覆盖为不同版本；`integration_test` 子模块也会请求默认版本。执行前需具备该 SDK 组件，不能因本机存在 36.1.0 就推定依赖已满足。

在 debug / release 或应用 / 集成测试入口之间切换时，保留 Flutter 默认的 pub 与平台文件生成流程，不加 `--no-pub`：当前 Flutter 在该选项下也跳过插件注册文件重新生成，可能使注册代码与 Gradle 的 dev 插件筛选不一致。生成文件由 Flutter 维护，不手改；构建后复核 lockfile 没有意外变化。

iOS 未签名 App 是真机目标构建产物，不能直接安装或当作真机运行证据。模拟器集成测试使用 debug Flutter runner 加载同一 Rust release 核心；release 包运行需要另行记录证据，不能把 debug 测试直接外推。`flutter build ios --simulator` 的通用架构构建不替代具体 arm64 目标验证；本实验没有引入 x86_64 iOS Rust target。

产物、合成运行报告和诊断放在 `build/`、`target/` 或 `local-evidence/`，不得提交。`integration_test` 的报告含合成别名及金额，没有密钥、真实账单或个人身份；私人参考与衍生材料不参与实验。

## 5. 检查与资源生命周期

`check-core` 执行真实 Rust fmt / test / clippy 及 C header 检查；`check-mobile` 执行锁定 pub、Dart 格式、分析、宿主 native 加载与错误 / UI 测试。PR 的 `Core Quality` 在 Linux 验证宿主复用这些入口，`Candidate Quality` 严格汇总它与 `Repo Hygiene`；CI 没有宣称覆盖移动端、真机或密码评审。实际命令与边界见[检查说明](../../scripts/README.md)。

后续直接复用现有 Android / iOS 模拟器，不默认创建项目专用设备或下载额外系统镜像。实验数据限定在本项目 App 沙盒；如验证确需新系统版本、干净设备或破坏性操作，先说明具体原因与影响再确认范围。

Android / iOS 模拟器、adb、Gradle 或 Dart 调试进程逐项记录创建归属与端口及启动前状态；只关闭本批启动且确认没有其他任务使用的实例，不关闭原已运行的共享设备。关闭 / 删除专用模拟器可以清除实验 App 容器，不能对用户设备或共享模拟器执行全局清理。下载包、构建工具和缓存可以保留在忽略目录供复现；结束必须停止本批进程并记录结果，不能遗留未知后台服务。


## 6. 压力、故障、并发与强杀恢复实验

`validation::Request` 是新增实验 JSON 的单一契约：显式 ABI、绝对临时目录与有类型的 action；未知字段 / action、超长请求与 ABI 不符均拒绝。薄 C ABI 的 `radishledger_probe_validate` 与 FRB 的 `run_validation` 共用同一个 Rust dispatcher，输入上限 4096 bytes、结果句柄所有权沿用第 2 节。合成 panic 在 Rust 边界捕获为 `internal_error`，随后必须仍能调用真实加密核心；不更换全局 panic hook，不把 OOM abort / 非法指针归为可恢复异常。

压力 runner 先核对薄 C ABI 的 16 句柄容量、短缓冲区不写、重复释放拒绝、无效 UTF-8 / 空指针 / 过长长度拒绝，再对每个 bridge 执行 200 次预热与 3 × 2000 次真实 native 错误返回。每批要求活跃句柄归零，记录进程 RSS；预热后到末批增长超过 64 MiB 则失败。这是短时异常增长检查，不是泄漏检测器、allocator 峰值、长期稳定性或真机内存结论。FRB 在一个工作 isolate 中初始化后执行各批，最后 dispose；并不宣称覆盖数千次 FRB runtime 初始化。

并发实验对同一随机密钥的 SQLCipher 库打开 4 个写连接和 1 个读连接。四个 writer 竞争同一笔 7 分转账，各自提交 16 笔 1 分转账，并重复回调每笔意图；最终只有 65 笔新转账，71 次重放。两笔期初为现金 10000 分、银行 20000 分，最终必须为 9929 / 20071 分，事实 / outbox / 序号均为 67。读连接在同一读事务内核对余额守恒与各表计数，不通过分离查询拼接伪快照。常规 `Store::commit` 和故障观测点共用唯一写入路径。

UI 集成测试在实际核心工作期间连点并停止等待，确认只调用一次、主 isolate 定时心跳继续、核心真实完成后晚结果不覆盖取消状态。它不声称底层事务被取消；更精细的帧延迟、KDF profile 与长时间压力仍需后续证据。

### 独立进程恢复

宿主检查通过 `m0-recovery-worker` 子进程执行；移动端由 `lib/recovery_main.dart` 与 `scripts/check-mobile-recovery.py` 配合，必须先安装该 runner 到指定的现有模拟器。控制文件只包含 bridge、随机 run 身份和中断点，放在本项目 App 的临时目录（当前 Flutter Android 为 code_cache，iOS 为 tmp；runner 输出实际路径元数据供核对），不读取其他 App 数据。脚本只接受 Android emulator 或明确的已启动 iOS Simulator；已有控制文件会阻断，不默认覆盖未知实验。

恢复 fixture 使用公开固定字节 `[0xA7; 32]` 作为非秘密测试材料，只能处理随机命名且带归属标记的合成库。它刻意让独立进程重开同一实验数据库，不能作为用户 key、生产 bootstrap、解锁或备份方案；不写入秘密文件，不操作 Keystore / Keychain，也不用于证明密钥保密性。正式密钥恢复仍由 M0-C 设计与验收。

外部 harness 等待 Rust 写入的 run / PID / 观测点标记，核对目标进程属于本项目 App 后发送 SIGKILL：

| 中断点 | 新进程读取时的独立预期 |
| --- | --- |
| 事实与 selected 已写、outbox 尚未写 | 两笔期初完整保留；转账不存在；序号 / 修订 / outbox 均为 2 |
| outbox 与序号已写、事务尚未 commit | 同上；未提交事务整体回滚 |
| commit 已完成、结果尚未返回 | 1700 分转账完整保留；余额为 8300 / 21700；序号 / 修订 / outbox 均为 3 |

新进程必须具有不同 PID，核对具体 operation identity 与两侧余额后重放同一意图，再重放一次；始终只有一笔转账，并拒绝复用同一操作身份提交 1701 分。没有到达标记或超过 45 秒未被外部强杀均失败，不以主动返回错误代替进程中断。成功后清理本次带归属标记的 fixture 和控制文件；失败保留合成材料供核对。该实验覆盖进程强杀，不代表断电 / 文件系统丢写、真实设备重启或秘密恢复。

```bash
# 保留第 4 节工具链环境；先启动选定的现有模拟器。
cd apps/m0_probe
flutter drive --driver=test_driver/resilience_test.dart \
  --target=integration_test/resilience_test.dart --device-id=DEVICE_ID
# 安装实验专用 runner；--no-resident 退出调试会话，App 等待合成命令。
flutter run --debug --no-resident --target=lib/recovery_main.dart --device-id=DEVICE_ID
cd ../..
python3 -B scripts/check-mobile-recovery.py android emulator-5568
# iOS 使用精确 Simulator UDID，不能传 booted 或真机标识。
python3 -B scripts/check-mobile-recovery.py ios IOS_SIMULATOR_UDID
```

两套集成入口分别保留 `*-resilience-report.json` 与 `*-recovery-report.json`，记录每个 bridge、RSS、读样本和三处强杀前后的事实。恢复 runner 会替换本项目实验 App 的入口；后续普通调试应重新运行 `lib/main.dart`，不据此更改正式产品入口。测试结束按进程归属关闭本次启动的模拟器 / adb，保留设备和其他 App。
