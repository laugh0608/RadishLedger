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

基础账务入口中，两个候选调用同一个 `run_probe(directory)`，输入只允许存在的绝对临时目录。Rust 在其下独占创建随机实验子目录，生成内存中的随机数据库密钥，执行[共同材料](../validation/accounting-cases.md)，核对实际结果后关闭并删除自身实验子目录。Flutter 不计算余额，不生成预期，也不传入数据库密钥。

领域层按规范整数分与 Gregorian 日期校验，转账使用一项事实表达两侧变动；余额、正负资产、月报和分类结果由有效修订派生。存储层保留修订、删除标记及冲突候选，在同一 SQLite 事务写入选中事实、outbox 和序号。重复 operation identity 只接受完全相同的意图；对账校验观察序号、观察余额和固定差额。

probe 使用真实 SQLCipher 事务：每次保存前在 outbox 插入点以测试触发器拒绝，核对事实、修订、序号和队列均回滚，再执行正常保存及重复提交。每个案例关闭后使用同一内存密钥重新打开并核对，另用随机错钥尝试打开。金额预期来自手写 JSON，集合输出在替换合成别名后排序以保持跨调用可比较。

基础入口是固定合成材料的接入实验，账户 / 分类由实验准备动作建立，数据库密钥只在本次 Rust 进程内存。第 6 节的跨进程事务恢复使用公开固定合成 key；第 7 节的 bootstrap 入口使用运行时随机秘密，接入独立材料跨重启解锁与系统保护 adapter。三个入口的材料和证据不能混用。它们均未提供生产账户创建、用户账簿、备份或迁移；schema `1`、C ABI `1` 和 JSON 返回结构都是实验契约，不能解释为生产格式冻结。

## 2. 薄 C ABI 契约

正式声明见 [radishledger_probe.h](../../crates/ledger-mobile/include/radishledger_probe.h)，Dart 调用见 [native.dart](../../apps/m0_probe/lib/src/native.dart)。下表描述基础 / 故障入口的普通输入与结果句柄；bootstrap 的秘密通道另见第 7.7 节，秘密不进入普通结果注册表。

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

在 debug / release 或应用 / 集成测试入口之间切换时，保留 Flutter 默认的 pub 与平台文件生成流程，不加 `--no-pub`：当前 Flutter 在该选项下也跳过插件注册文件重新生成，可能使注册代码与 Gradle 的 dev 插件筛选不一致。生成文件由 Flutter 维护，不手改；构建后复核 lockfile 没有意外变化。同一 `apps/m0_probe` 工作目录中的 Android / iOS 构建、测试及 debug / release 模式切换必须串行执行：它们会改写共享的 Flutter 插件注册状态，跨平台并行也可能使集成测试插件缺失。

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

## 7. M0-C bootstrap、解锁与独立恢复方案

本节描述 bootstrap 实验实现、复现契约与验收要求；实际取得的证据和未关闭门槛统一看 current / devlog。沿用[威胁模型](../privacy-threat-model.md)的候选算法和密钥层次，不冻结生产格式、最低系统、bridge 或 KDF profile。实验仍只接受合成账务，但密钥在运行时随机生成，不能沿用 M0-B 的公开固定 DB Key。

### 7.1 最小闭环与依赖范围

闭环为：创建独立恢复材料 → 持久化 bootstrap → 创建加密库 → 确认恢复材料并核对 → 发布可用状态 → 保存共同账务 → 结束进程 → 解锁原库 → 使本次系统保护项不可用 → 独立恢复原库 → 核对未同步事实 → 重建日常保护 → 再次重启解锁。系统日常保护未就绪与账库独立解锁可用分别报告。

Rust 继续承担随机数、派生、认证封装、bootstrap 状态、SQLCipher 和账务核对；Swift / Kotlin 只适配系统秘密存取及认证错误，Flutter 只呈现流程和调用。两个 bridge 共用 Rust 契约，不把秘密放入现有普通 JSON 结果注册表或报告。跨语言秘密使用有界字节缓冲区、最短借用和显式清理，平台 / Dart 不可保证清零的副本需记录，不承诺绝对内存擦除。

本批经项目所有者确认引入以下固定直接依赖；仅启用派生、封装、分配及必要清理 features，不引入随机源替代现有 `getrandom`，不启用不需要的并行 KDF。依赖变更须复核传递包、MSRV、features、许可、已知安全公告及移动目标，并同步 lockfile、第三方清单与包内 notices；解析失败不自动换版本。锁定、编译和许可收集分别提供对应证据，不能据此宣称已完成安全公告或完整供应链审查。

| 候选 | 用途与来源 | 许可 |
| --- | --- | --- |
| chacha20poly1305 `0.11.0` | [RustCrypto AEAD](https://docs.rs/crate/chacha20poly1305/0.11.0)，只使用 XChaCha20-Poly1305 | MIT OR Apache-2.0 |
| argon2 `0.6.0` | [RustCrypto Argon2](https://docs.rs/crate/argon2/0.6.0)，只使用 Argon2id v19 | MIT OR Apache-2.0 |
| hkdf `0.13.0` | [RustCrypto HKDF](https://docs.rs/crate/hkdf/0.13.0)，用途隔离派生 | MIT OR Apache-2.0 |
| sha2 `0.11.1` | [RustCrypto SHA-2](https://docs.rs/sha2/0.11.1/sha2/)，HKDF-SHA256 与非秘密文件核对 | MIT OR Apache-2.0 |

候选采用 RustCrypto 的共同实现，避免两端各选一套密码库；不增加 Flutter 安全存储插件或全局工具。四个直接依赖及其必要传递依赖需要下载缓存并改变 Cargo 声明 / lockfile，是本方案新增的实施影响；不顺带升级已有组件。依赖缓存和多目标编译产物可保留在既有忽略目录，实际声明与传递版本以 Cargo.toml / Cargo.lock 为准，许可已由固定缓存重新收集。

### 7.2 封装与资源边界

- 每次创建独立生成 32-byte Root Key、Device DB Key、Recovery Secret，以及随机 vault / device 身份；该切片不预建 Book / Object / Archive 功能。
- 口令经 Argon2id v19 派生，再以固定用途标签派生封装钥；Recovery Secret 经独立 HKDF 标签派生另一封装钥。两条路径分别认证解封同一 Root Key，再由 Root Key 派生本机 DB 封装钥、解封原 Device DB Key。系统路径只保护独立随机的本机解锁钥，用它封装 Root Key，系统删除不改变独立恢复关系。
- XChaCha20-Poly1305 每次封装使用新 24-byte 随机 nonce，认证成功后才交付明文。AAD 使用明确的定长字段和长度前缀编码，绑定实验格式、算法、用途、vault / device 身份、封装代次；口令封装还绑定 profile、salt 与完整参数。JSON 如用于磁盘外壳，不能直接充当 AAD 编码规范。
- 初始运行 profile 只接受 64 MiB、3 次迭代、1 路、32-byte 输出及 16-byte 随机 salt；未知、减弱、超大、溢出参数在分配前拒绝。独立算法向量用专用测试入口，不使较弱向量参数成为运行时可接受 profile。
- bootstrap 读取上限 16 KiB，固定封装数量与字段长度；口令按原始 UTF-8 字节处理，不 trim 或隐式 Unicode 归一化，实验输入上限 1024 bytes。资源上限不是对任意弱口令安全性的承诺。
- 恢复文本包含实验版本、完整随机秘密和输入校验；校验仅发现录入错误，最终身份和正确性仍靠 AEAD。恢复资料包含 vault 身份及必要封装信息，秘密不默认复制、落盘或备份。它只能解锁已有数据，不冒充完整账务归档。
- 同时只执行一个 KDF；后台 / 取消使会话代次失效，晚完成结果不得打开会话或更新 UI。底层不支持中途停止时明确显示仍在结束工作，完成后清理并丢弃结果，不把“停止等待”说成已停止计算。

### 7.3 bootstrap 的持久化与中断

bootstrap 位于 App 私有、排除自动备份的独立目录，与 DB 分开；目录 `0700`、普通文件 `0600`。不写明文根钥、DB Key、口令或恢复秘密。系统保护引用可缺失，但独立封装不得只存入尚不能解锁的 DB。平台备份排除同时覆盖 DB、WAL、临时封装和状态文件，不能只排除主 DB。

创建使用独占随机暂存目录，文件写入、同步和同一文件系统的发布有明确顺序。活动账库存在时拒绝重新创建；并发创建只能发布一个活动身份，不能依赖进程内锁证明跨进程排他。发布失败保留可诊断暂存，不替换旧活动账库。

| 中断边界 | 重启后的行为 |
| --- | --- |
| 独立封装完整落盘之前 | 标记创建未完成；没有可用账库，不宣布成功；保留现场，不自动覆盖重建 |
| 封装完成、DB 尚未建立 | 用原独立材料认证后继续原创建身份；只有确认 DB 从未建立时才能创建，不产生第二套钥 |
| DB 创建开始但未完成 | 先尝试打开并核对身份 / schema；损坏或部分 schema 明确失败，保留文件，不自动删库 |
| DB 已核对、恢复材料未确认 | 保留待确认状态，不允许新增账务；重新认证恢复材料后继续 |
| 系统项已创建、活动状态未发布 | 通过本次精确引用核对孤立项；系统操作幂等且不得覆盖同名旧项；独立恢复路径仍完整 |
| 发布之前 / 之后 | 重启只有未发布暂存或同一完整活动身份；发布完成后强杀应读到同一 DB 与封装 |

活动标记不是单独的可信依据：解锁后核对封装认证身份、DB 内受加密保护的身份和 schema。失败不得新建空库。错误材料和损坏的 bootstrap 在碰 DB 前拒绝；需要检查 DB 的失败场景使用只读核对或保全副本，不能让失败验证修改旧文件。文件同步失败是未定 / 失败状态，不宣称掉电持久化成功；SIGKILL 不替代断电实验。

### 7.4 系统保护、失效与重建

| 平台 | 实验适配策略 | 能力与停止线 |
| --- | --- | --- |
| iOS | Keychain 项采用 `WhenPasscodeSetThisDeviceOnly` 与 `userPresence`，不启用同步；仅保存本机解锁钥 | 无设备密码或访问控制能力时报告不支持；不改为无认证项。Simulator 成功不证明 Secure Enclave 或真机保护 |
| Android | Android Keystore 不可导出 AES-256-GCM 钥封装本机解锁钥；系统生成 IV，认证绑定用途 / 身份；要求系统用户认证 | 首个具体实验路径针对已有 API 35，采用 API 30+ 认证参数；旧 API 明确未支持，不提高正式最低系统。记录实际 KeyInfo 等级，不预设 StrongBox / TEE |

平台策略依据 [Apple Keychain 认证示例](https://developer.apple.com/documentation/localauthentication/accessing-keychain-items-with-face-id-or-touch-id)与 [Android KeyGenParameterSpec](https://developer.android.com/reference/android/security/keystore/KeyGenParameterSpec.Builder)。系统认证必须实际限制钥的取用，不能用一个与密钥操作无关的成功弹窗代替。缺锁屏 / 生物识别条件时可验证明确拒绝及独立口令恢复；该结果不能记成日常系统解锁通过，也不自动修改共享模拟器的安全设置。

返回状态至少区分用户取消、认证拒绝、暂不可取、项缺失、已失效、不支持、认证密文损坏和平台错误；保留非秘密平台错误码。平台无法区分某些原因时如实返回已知类别，不凭空推断。取消不删除钥、不切换账库、不自动触发恢复。

失效实验只删除本次创建的精确 alias / Keychain 项，验证“保护项缺失后的恢复”；它不等于已经验证移除锁屏密码、重新录入生物识别造成的真实失效。恢复成功后先核对原库事实、序号、outbox、候选和删除状态，再创建新代次日常保护并实测读取。原独立封装始终保留；新项及新封装均持久化并核对后才切换引用，最后按精确旧引用清理。任一步失败仍能通过原独立材料重试，不轮换 DB Key 或重新建库冒充恢复。

### 7.5 验证层次与完成判据

| 层次 | 必须取得的证据 | 不外推的结论 |
| --- | --- | --- |
| Rust 算法 | [RFC 5869](https://www.rfc-editor.org/rfc/rfc5869) HKDF、[RFC 9106](https://www.rfc-editor.org/rfc/rfc9106) Argon2id、[XChaCha 草案](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-xchacha-03)公开向量；逐字段篡改、截断、错误材料、极端参数拒绝 | XChaCha 草案不是正式 RFC；同实现 round-trip 不能替代向量；向量不证明整体安全 |
| Rust 持久化 | 每个 bootstrap / 保护重建中断点、并发创建、短写 / IO 拒绝、文件与原身份保留；新进程独立材料开库 | 宿主文件系统不替代移动系统；强杀不替代断电 |
| 两端模拟器 | 两种 bridge 共用流程；创建、真实新 PID 解锁、错误输入拒绝、系统项缺失后恢复、重建后再次重启；实际 UI 取消 / 后台及晚结果抑制 | 系统路径不支持时逐项记阻塞，不能用 mock、低保护项或跳过断言获得整批通过 |
| 账务保真 | 使用共同样例的独立预期，比较事实 / 修订 / 删除 / 候选 / outbox / 序号与余额；验证未同步事实未丢失、重放不重复 | 同一次内存会话重开不能代替跨进程；余额相等不能证明事实完整 |
| 文件与运行资源 | 检查 bootstrap、DB、实际生成的 WAL / journal / 临时文件；记录 KDF 耗时与进程峰值、串行限制及取消 UI 响应 | 字符串未检出不是全部密码证据；模拟器时间 / RSS 不冻结真机 profile |
| 后续真机门槛 | release 正常 / 取消 / 拒绝 / 实际失效 / 设备重启 / 后台、硬件等级、KDF 时间和峰值内存 | 本次继续复用模拟器，未签名 iOS 或 Android APK 构建不算真机验收 |

测试口令和恢复秘密运行时生成，仅由测试控制器内存持有并经不记录载荷的受限通道输入；不得通过命令参数、环境变量、日志、截图、报告或提交传递。测试控制器退出后不保证重新取得测试秘密，因此先完成全部恢复核对再清理。本批报告只留状态、非秘密身份与文件核对值；公开算法向量与合成金额可以进入测试材料，运行时秘密不能留作“复现证据”。

### 7.6 执行对象与收尾

复用现有 Pixel_9_Pro 和 iPhone 17 Pro（前次 iOS UDID `CB767BAB-1B1B-4AF8-A3FB-0C2B793A0756`），执行前重新核对版本、ID、进程归属与能力。Android 预计沿用空闲的 console port `5568`；端口被占用则先识别，不终止他人进程。无需服务端或额外网络服务。

系统项限定在本项目 App 沙盒 / entitlement 内：Android alias 前缀 `radishledger.m0c.`；iOS service `com.radishledger.m0c`、account 为本次随机运行身份及代次。创建前核对不存在，清理使用完整身份，禁止按前缀批量删除、访问宿主登录钥匙串、清空模拟器或操作其他 App。删除测试保护项是故障实验的一部分，只适用于本次新建项。

依赖解析 / 下载使用 Cargo 固定版本和既有缓存；按原入口生成 bridge、收集 notices、执行 `check-core` / `check-mobile` / `check-repo`、两端目标构建与集成实验。预计下载和首次多目标编译需要数十分钟，模拟器实验另需数十分钟，以实际耗时为准；单次 KDF 工作内存候选为 64 MiB，整进程峰值另测。证据保存在忽略的 `local-evidence/m0-c/`，构建缓存沿用既有目录。

结束时先保存不含秘密的验证摘要，再精确清理本次合成账库、暂存及系统项，确认不存在本批孤立项；恢复 App 普通入口。只关闭本次启动的模拟器 / adb / 构建进程，保留设备定义、其他 App 和已有证据。检查后本地提交，不 push、发布或部署。新增依赖和本项目隔离系统项已获授权；共享设备锁屏设置保持不变。实际通过与未验证项记录在 current / devlog，不把实施方案当作整项验收。


### 7.7 已接入的复现入口与证据边界

`crates/ledger-core/src/bootstrap.rs` 是实验状态机，`crypto.rs` 执行固定算法与独立向量，`bootstrap_files.rs` 负责精确目录归属、发布和锁。Rust 1.96 的标准文件锁没有 Android 实现，因此 Android 显式调用现有固定 libc 的 NDK `flock`；其他目标使用标准库，锁占用与 IO 错误分别返回。没有在 Android 跳过排他。

独立恢复材料为 `RLM0C1-` + 64 个十六进制字符 + `-` + 8 字符校验。UI 生成后显示一次，要求重新输入确认；未确认且没有账务的账库可用正确口令重新生成恢复材料，替换旧的未确认封装。已确认账库拒绝该入口。创建中途失败保留现场；DB 已开始但不存在时返回 `missing_database`，不自动新建。新建合成账库的保真 fixture 使用 A11，恢复前后比较身份、完整业务表摘要及审计 / outbox；旧 A01 记录与既有材料不自动迁移，也不能作为 A11 的运行证据。

两条 bridge 的秘密走独立有界字节输入 / 输出，不放入普通 JSON 结果句柄。薄 C ABI `radishledger_bootstrap_execute` 的 metadata 上限为 4096 bytes、秘密输入上限为 1104 bytes，调用方提供至少 128 bytes 的独立秘密输出缓冲区及实际长度指针；普通结果句柄只返回非秘密 JSON。普通句柄容量不足时不执行有副作用的核心动作，调用方负责清理秘密输入 / 输出，不能将第 2 节的 UTF-8 JSON 约束套用到随机秘密字节。平台 codec 可返回只读缓冲区，Dart 复制到本方可清理的字节数组；Swift Data / CFData、Dart 字符串和 codec 内部副本不承诺全部擦除。UI 锁定 / 取消使晚完成结果失效，核心工作结束前禁止重入；不宣称 Argon2 已被中途停止。

在已授权、已启动的现有模拟器中安装控制入口。Android 构建须显式限定 arm64；iOS 使用指定 arm64 Simulator 的 `flutter run --debug --no-resident -d <UDID>`，本机通用 simulator 构建的 Flutter 双架构检查失败不能作为业务失败。两者目标均为 `lib/bootstrap_main.dart --dart-define=M0C_DRIVER=true`。随后从仓库根运行：

```sh
python3 -B scripts/check-mobile-bootstrap.py android emulator-5568
python3 -B scripts/check-mobile-bootstrap.py ios CB767BAB-1B1B-4AF8-A3FB-0C2B793A0756
```

控制器不启动设备，要求准确设备和本项目包。可用 `--output-dir local-evidence/m0-c/<batch>` 分批保留报告；解析后的路径必须位于项目 M0-C 忽略证据目录内。秘密只在内存和 `0600` 私有 FIFO 中传递；Android 写入用 `adb shell -T` 的定长 `dd`，读取用 `exec-out`，避免 raw exec 的输入方向 / 完成语义造成短帧。帧短读、长度错误和超时立即失败，不输出载荷。脚本结束逐个清理登记的随机账库及系统引用；失败时保留非秘密 ownedRuns 报告，后续仅按这些身份处理。不能覆盖失败报告后忘记残留对象。

实际 UI 验证入口为 `integration_test/bootstrap_test.dart`，驱动为 `test_driver/bootstrap_test.dart`，复用常规 `flutter drive --driver=... --target=... -d <device>`。Android 可先限定 arm64 构建 integration target，再通过 `--use-application-binary=<apk>` 安装同一产物。它调用真实核心检查错误口令、锁定取消、晚结果抑制、重试、A11 独立计数及原库完整摘要 / 审计，截图只在输入已清空的合成结果页生成。设置 `M0C_EVIDENCE_SUBDIR=<batch>` 可将 UI 报告和截图写入同批子目录；只接受字母、数字、下划线和连字符组成的单个名称。宿主 widget / platform 测试另验后台状态和只读 codec；模拟生命周期不代替系统任务预览或真机后台证据。

报告与截图写入忽略目录 `local-evidence/m0-c/`。schema 2 每端核心报告包含两条 bridge ×（正常重启 + 六个故障点 + A11 非空 WAL 强杀 / 原意图重放），共 16 组；另有每条 bridge 四类文件 × 六种模式，共 48 组文件故障。恢复报告对照 A11 人工预期，并比较身份、完整摘要、审计及 ready；结束时复核平台能力未变。`protectionPrepared` / `protectionPublished` 及文件故障中的核心 local-wrap 恢复单列，不冒充系统认证。实际平台测试根据现有能力执行：iOS Simulator 可核对精确 Keychain 项删除与重建；Android 无认证条件时明确记录 unsupported，日常保护与实际失效仍未验收。SIGKILL 与显式故障注入不证明断电或实际磁盘故障恢复。

### 7.8 非空账务与文件发布故障

复用共同材料 A11，不另造第二套账务写入路径。`seed` 在所有人工预期检查通过后保存完整表摘要；新增 `fixtureSaved` 强杀观测点保持写连接打开，使新进程必须读取实际残留 WAL。`replay` 先认证并核对完整基线，再从原 outbox 取出原意图调用同一 `Store::commit`，要求每条都返回 `Replayed`。它只用于隔离合成实验，不是同步上传或生产恢复接口。

文件故障通过每个请求的可选 `file_fault` 显式注入，不用全局状态、环境变量或第二套发布实现。`file` 仅接受 `bootstrap`、`ready`、`localProtection`、`activeProtection`；`stage` 及判据如下：

| stage | 实际动作与必须观察到的结果 |
| --- | --- |
| `shortWrite` | 每次最多向真实临时文件写 7 bytes；同一 `write_all` 必须续写完整，再同步和发布，结果仍可解锁 |
| `writeZero` / `writeError` | 已写入首段后返回零写 / 注入 IO 错误；拒绝发布，原文件集合不变，临时文件由本次作用域回收 |
| `fileSync` / `publish` | 在文件同步 / 原子发布边界注入拒绝；不能返回成功或覆盖旧目标 |
| `directorySync` | 已原子发布后注入目录同步失败；返回 `bootstrap_publish_uncertain`，不得声称旧引用未变或掉电持久化成功；新进程核对已出现的完整新文件并通过独立材料继续 |

真实目录同步错误与注入使用相同的“发布结果未定”分类，并保留可取得的 OS 错误码。上述错误注入验证控制流和文件保全，不宣称操作系统真实发生了磁盘故障；DB pager 注入见第 7.9 节，断电及文件系统丢写仍需单独验证。

宿主 `bootstrap_process` 覆盖四类文件 × 六种模式、原六处强杀、新增 WAL 强杀、错误口令 / 合法格式错误恢复材料、损坏封装与重放。首次封装未发布时保留原目录且拒绝覆盖创建；已发布而目录同步未确认时可用原材料恢复原身份。日常保护重建失败后以独立材料核对原账库，再重试切换并在新进程读取。核心 local-wrap 成功不代表真实系统认证通过。

`store_file_tests` 实际观察 SQLCipher DB / WAL / SHM、checkpoint 和显式 DELETE 模式下的回滚日志；用未加密合成库作标记扫描正向对照，并核对普通无 key 连接 / 错钥拒绝与回滚后完整事实。默认仍是 WAL、`synchronous=FULL`、`temp_store=MEMORY`；有界排序查询与实验目录清单检查不等于完整临时目录、索引、缓存、导出或内存审计，标记未检出也不是完整密码安全证明。

复现继续使用 `check-core.sh` 与 `check-mobile.sh`。定向入口为 `cargo test --locked -p radishledger-core --test bootstrap_process`、`cargo test --locked -p radishledger-core store::file_tests` 和在 Flutter 工程内执行 `flutter test test/bootstrap_native_test.dart`。Flutter 宿主测试实际调用两种 bridge，验证错误分类、保真与恢复；跨 PID 证据来自 Rust 子进程测试，不从 Dart isolate 推断。

设备端沿第 7.7 节入口运行同一文件发布实现，每种故障后重启 App，并对照独立材料读取及原文件集合；活动保护切换还需重试后再次重启读取。A11 在 `fixtureSaved` 停住时实际读取非空 WAL 大小，再 SIGKILL、新 PID 解锁及原意图重放。移动报告覆盖封装文件发布与 WAL 恢复；宿主 DB / SHM / 回滚日志扫描及第 7.9 节结果未因此成为移动端 pager / 临时文件证据。

### 7.9 DB pager 故障与临时文件边界

`store_pager_tests.rs` 与 `store_test_vfs.rs` 仅由 `cfg(all(test, unix))` 接入，不进入生产核心、bridge 或移动包。父测试在独占子进程注册观察 VFS，原 VFS 继续执行真实文件 IO、锁、SHM 与 mmap；父进程及其他并行测试不切换默认 VFS。随机合成 DB key 仅通过 stdin 传递，不写参数、环境变量、报告或日志。

先取得同一 A11 账库在 WAL 提交、DELETE 日志提交和 checkpoint 中的成功 IO 序列，再逐个写入 / 同步位置注入；每次核对文件类别、调用序号、长度 / flags 和偏移，未命中不能通过。SQLite 的 `xWrite` 返回错误码而非已写字节数，所以短写场景实际调用原 VFS 写入严格少于请求的 2–7 bytes，再返回 `SQLITE_IOERR_WRITE`；不把正数当作 SQLite 的成功返回。`xSync` 分别在调用父同步前拒绝，以及父同步成功后报告 `SQLITE_IOERR_FSYNC`，覆盖可能已经持久化但调用方收到失败的情况。此处验证的是 pager 面对 IO 失败的行为，不证明内核实际发生短写 / fsync 故障或掉电。

每次操作返回后保持连接打开，父进程发送 SIGKILL 并等待实际退出；随后父进程通过普通 `Store::open` 恢复。A11 候选、删除和 outbox 必须保持，新增 123 分转账只能完整存在或完全不存在：余额为旧的 10000 / 8000 分或新的 9877 / 8123 分，净资产均为 18000 分。核对人工计数、完整摘要、审计及 `integrity_check` 后，用相同操作身份重试并再次重放；不得重复入队。checkpoint 失败必须保留此前已提交的完整新事实。同步报错不能推定事务没有提交。

临时文件验证使用 8192 行合成数据和受限页面缓存，按下表建立实际工作负载与观测。`xOpen` / `xWrite` 记录文件类别和阶段；`xClose` 前通过原 VFS 分块扫描仍打开的真实文件，包含已经 unlink、目录枚举看不到的匿名文件。单文件扫描上限 64 MiB，超界或读取失败均阻断；不截断后宣称通过。SHM 由 `xShmMap` 单独计数，并在连接仍打开时扫描实际 sidecar。

| 文件 / 操作类别 | 判据与正向对照 |
| --- | --- |
| DB、WAL、回滚日志、SHM | 真实文件 / 映射存在；加密构建的合成明文标记不应出现在实际写入与文件扫描中 |
| TEMP 表、排序、自动索引、物化查询、索引创建 | 原 `temp_store=MEMORY` 不打开磁盘临时文件；相同构建的隔离 FILE 对照必须在每一阶段实际落盘。另核对排序结果、查询计划、排序计数、自动索引行数与物化结果 |
| 语句回滚 / sub-journal | 大批更新最后触发 UNIQUE / ABORT，先前更新全部撤销；FILE 对照必须观察非空 sub-journal，不能只验证最终表内容 |
| VACUUM | 验证重建与 A11 保全；FILE 对照观察真实临时 DB，逐个记录是否包含标记，不假设所有临时文件都明文或都加密 |
| 多库提交 / super-journal | 仅在本次合成目录附加独立加密库，DELETE 模式下同时提交两库，观察非空 super-journal 与清理；这不是生产 ATTACH / 多库事务能力声明 |

FILE 对照会在 TEMP 表、排序或索引等临时文件检出合成明文，这是用于验证观测有效性的预期结果；不能据此放宽生产 MEMORY 策略。SQLite 对临时文件类别与生命周期的说明见[临时文件](https://www.sqlite.org/tempfiles.html)、[VFS 契约](https://www.sqlite.org/c3ref/vfs.html)及 [IO 方法](https://www.sqlite.org/c3ref/io_methods.html)；SQLCipher 的临时存储保护边界见[官方设计](https://www.zetetic.net/sqlcipher/design/)。测试实现为本项目编写，仅使用已锁定绑定，不复制第三方 VFS 示例或新增依赖。

日常入口为 `check-core.sh`。定向运行：

```sh
CARGO_NET_OFFLINE=true cargo test --locked -p radishledger-core --lib store::pager_tests -- --nocapture
```

可将 `M0_PAGER_EVIDENCE_DIR` 指向已创建的 `local-evidence/m0-c/<batch>`，保留不含 key / 页面载荷的 JSON；解析后的目录不能越出项目 M0-C 证据范围。两个父测试实际执行全部子进程，标为 ignored 的 `pager_worker` 仅是专用子进程入口，不是跳过故障覆盖。每个子进程最多等待 45 秒；超时 / 失败只终止本次 PID 并等待回收。`TMPDIR` / `SQLITE_TMPDIR` 仅在子进程指向本次独占目录，正常完成先关闭 / 回收进程与文件，再清理目录并保存成功报告。

该切片覆盖宿主有界工作负载中的 SQLite 临时文件类别，不等于所有查询形态、移动平台、长时缓存、OS swap / crash dump、导出或归档边界已经验证。现有产品未开放任意 SQL、ATTACH 或导出入口；新增路径需补相应文件审计，不能直接沿用本批通过结论。
