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
| iOS | arm64 真机与 arm64 Simulator；App deployment target `15.0`；每批记录 `xcrun` 实际选择的 Xcode / SDK，历史构建版本不代表当前环境 |
| 存储 | rusqlite `0.41.0`、libsqlite3-sys `0.39.1`、sqlcipher-src `419.0.6`，内含 SQLCipher `4.19.0 community` / SQLite `3.53.4` |
| 密码底层 | Apple 使用 CommonCrypto；Android / Linux 使用静态 OpenSSL；当前 `4.0.3` 加 SVE 前置条件补丁，来源和目标验证见第 3.2 节，不依赖开发机动态 libcrypto |
| Dart code assets | hooks `1.0.2`、code_assets `1.0.0`、ffi `2.2.0`；`pubspec.lock` 固定传递依赖 |

实验采用 Dart build hook 构建 Rust release code asset。构建入口 [build-mobile-core.py](../../scripts/build-mobile-core.py) 根据真实目标选择 Rust triple，传递 NDK clang / ar / ranlib 或 Apple SDK 与 deployment 参数；不支持的目标明确拒绝，缺库不能生成空资产或使用另一平台库代替。Flutter 当前传给 hook 的 iOS native 最低版本可能低于 App 的 deployment target，最终 App 安装下限仍为 `15.0`，不据此承诺较旧系统可用。

两项兼容约束留在正式来源：`libc = 0.2.189` 是 FRB 所含 backtrace 在 iOS 的兼容候选，`0.2.190` 的 dyld 声明限制已实测阻断 iOS；Rust release 暂不 strip，避免 Apple 27 SDK 下 [Rust / LLVM LINKEDIT 对齐问题](https://github.com/rust-lang/rust/issues/157750)。解除约束需重新通过宿主和移动目标的真实加载，不能只看编译成功。

模板来源、第三方许可与归属见 [THIRD_PARTY_NOTICES](../../THIRD_PARTY_NOTICES.md)。所有依赖升级、最低系统冻结和 bridge 采用结论另按实际证据评审，本篇不替代 ADR。

### 3.1 原生依赖修补候选与验证范围

原生源码包可能改变数据库版本、构建选项、密码提供方和许可材料，不能仅按 Cargo 包级公告判断修补是否完成。2026-10-25 评审的 SQLCipher 链已获项目所有者确认并接入，OpenSSL 后续候选见第 3.2 节；实际验证状态以[当前进度](../status/current.md)为准。

| 项目 | 已核对的候选 | 影响与采用条件 |
| --- | --- | --- |
| SQLCipher 绑定链 | rusqlite `0.41.0` → libsqlite3-sys `0.39.1` → sqlcipher-src `419.0.6+sqlcipher-4.19.0-sqlite-3.53.4-libtomcrypt-1.18.2` | SQLCipher `4.19.0` / SQLite `3.53.4`；新增独立源码包，不能当作原包内仅替换一个 C 文件。发布包分别声明 Rust 1.89.0 / 1.88.0 / 1.81，既有 1.96.0 满足 |
| 密码提供方 | Apple CommonCrypto 与 Android / Linux vendored OpenSSL | 保持提供方及现有 feature 选择，该 SQLCipher 批次不启用 libtomcrypt、系统动态 libcrypto、SQLCipher 5 beta 或改变 OpenSSL 版本；源包包含 libtomcrypt 不等于目标已编译它 |
| OpenSSL | 官方 `3.6.5` 有安全修复；crates.io 的 openssl-src 300 系列最新为 `300.6.1+3.6.3` | openssl-sys `0.9.117` 要求 openssl-src `^300.2.0`，不能直接选择已发布的 `400.0.2+4.0.3`。后续已批准的最小本地 patch 与迁移停止线见第 3.2 节，不能修改缓存绕过 |

来源为 [SQLCipher 4.19.0 公告](https://www.zetetic.net/blog/2026/09/08/sqlcipher-4.19.0-release/)、[rusqlite 0.41.0 发布](https://github.com/rusqlite/rusqlite/releases/tag/v0.41.0)、[sqlcipher-src](https://github.com/LucaCappelletti94/sqlcipher-src)、[OpenSSL 3.6 发行记录](https://openssl-library.org/news/openssl-3.6-notes/)及 crates.io 发布索引 / 包。rusqlite 发布说明中的 4.17.0 条目不是实际源包版本的充分证据；需核对锁定链、源码常量与运行报告。源包 SHA-256 匹配 registry 不等于已经独立复现源码生成或验证所有上游签名。

新 sqlcipher-src 由独立维护者发布，声明 `MIT AND BSD-3-Clause AND blessing AND WTFPL`；包装代码 MIT、SQLCipher BSD-3-Clause、SQLite public domain，内含 libtomcrypt 许可提供 public domain / WTFPL 二选一。已按现有保守许可并集保留随包材料，并明确 Android / iOS 不编译 libtomcrypt。notices 生成器已改为从 sqlcipher-src 收集嵌套许可及 sqlite3.h 的完整首段声明，缺失、空文件或格式漂移明确失败；对应回归随仓库检查运行。新增源码供应者与许可材料已按根协作规则确认，不改变项目本身许可证。

实施范围：精确调整两端 rusqlite 声明、锁定上述绑定链，核对完整依赖图；更新许可生成入口及必要契约回归；仅适配实际受影响的 API。保持密钥、KDF、schema、同步 / 备份格式与 bridge 契约不变。rusqlite 0.41 的 pragma helper 查询 SQLite pragma_list，不能识别 SQLCipher 的 cipher_version；Store 与观测路径统一使用固定 `PRAGMA cipher_version` 查询，缺失 / 空值 / 执行错误继续拒绝，无回退。本批依赖图的最低 SQLite feature 从 `min_sqlite_version_3_34_1` 升为 `min_sqlite_version_3_45_3`；上游 build.rs 移除 `SQLITE_ENABLE_MEMORY_MANAGEMENT`，其余显式 `-D` flag 集合未变。保存宿主实际 compile options，并完成 pager / 临时文件回归；旧宿主报告只存 TEMP_STORE 子集，不能宣称全部运行配置逐项等价。编译通过不代替数据库兼容或移动设备验证。

该批验证至少包括：

1. 用升级前已锁定核心建立含候选、删除、修订与 outbox 的合成旧库，分别保留正常关闭与中断后待恢复状态；由新核心打开、校验独立预期、完成恢复和原意图重放。单版本恢复用例不代替旧库兼容，若未验证回退则明确不承诺回退读写。
2. 复用密码 / 错钥拒绝、文件密文与明文对照、WAL / DELETE / checkpoint、pager 故障、TEMP_STORE 与清理重试检查；读取实际 SQLCipher / SQLite、密码提供方和编译选项，记录与旧基线的差异。
3. 运行 Rust 与 Flutter 宿主检查、双桥接压力 / 并发 / 错误 / 恢复、两端 arm64 native 构建及许可 / 治理检查。宿主与 native 构建作为本地候选证据；移动端实际运行、真机文件保护和系统认证仍按既有矩阵补验，不外推通过。

跨版本用例及旧 worker 保存要求见[检查说明](../../scripts/README.md#移动构建生成与证据)。该用例在默认检查中明确 ignored，只有显式提供旧 worker 才执行；不把同版本恢复计作旧库兼容。另有实际版本 / 密码提供方回归，要求 Apple 为 CommonCrypto、非 Apple 为对应批次锁定的 OpenSSL；平台未执行时不能宣称其运行通过。

SQLCipher 候选源码与日志只在忽略的评审目录和 Cargo 缓存保留；第 3.2 节另行批准的 openssl-sys 最小源码副本有明确来源清单。未通过来源、许可、编译或恢复验证时停止采用，保留真实原因。

### 3.2 OpenSSL 修补方案评审与验证计划

2026-10-25 评审后，项目所有者已批准 **OpenSSL 4.0.3 的受控迁移**及仅调整构建依赖约束的 openssl-sys 最小本地补丁。候选曾因 Android 初始化 SIGILL 停止采用并保存为 `72d13b0`。隔离修复证据保存为 `1d000cf` 后，项目所有者明确批准完整 openssl-src 与两行补丁正式接入、哈希约束的精确文本例外和正式工作区复验。当前源包与治理约束见[源码说明](../../third-party/openssl-src-patch.md)；历史失败保留在下文，正式接入结果另列。保留 Apple CommonCrypto、Android / Linux 静态 OpenSSL，不调整 SQLCipher 4.19.0 / SQLite 3.53.4、业务加密、KDF、schema、bridge 或支持矩阵。

#### 已核对的约束

- [官方发行与支持期限](https://openssl-library.org/source/)列出 3.6.5 支持至 **2026-11-01**，4.0.3 至 **2027-05-14**；截至本次评审，3.6 只剩一周公开支持。4.0 也不是长期支持版本。官方另已于 2026-10-23 发布 4.1.0，但本次 crates.io 索引尚无对应 openssl-src；不能把 4.0.3 称为最新 OpenSSL。
- 重新读取 [openssl-src 索引](https://index.crates.io/op/en/openssl-src)和 [openssl-sys 索引](https://index.crates.io/op/en/openssl-sys)：未撤回的 300 系列最高为 `300.6.1+3.6.3`，4.0.3 对应 `400.0.2+4.0.3`；openssl-sys 最新仍为 `0.9.117`。当前发布包要求 openssl-src `^300.2.0` 且启用 `legacy`，所以不能仅用 cargo update 选择 400 系列，添加一个未被选择的 patch 也不解决范围约束。
- 本地已锁定 openssl-sys 0.9.117 的 build/main.rs 有 `Openssl4xx` 分支，build/cfgs.rs 有 `ossl400`；这与 vendored 的 300 系列约束是两个问题。能识别 4.x 不等于已验证 SQLCipher、整个 FFI 绑定或 Android 构建兼容。
- SQLCipher 4.19.0 已锁定源码的 OpenSSL provider 使用 AES-256-CBC、EVP_MAC HMAC、PKCS5_PBKDF2_HMAC、RAND_bytes / RAND_add 与版本 / 错误接口；当前 Store 通过 sqlite3_key 传入 32 字节材料，不是 SQLCipher hex raw-key 语法，需保留 PBKDF2 路径验证。外层口令封装与恢复材料仍由 RustCrypto 实现。
- [3.6.4 / 3.6.5 发行记录](https://openssl-library.org/news/openssl-3.6-notes/)包含多项安全修复，3.6.5 最高为 High；其中网络协议、证书、EC / SM2 和 AEAD 等路径在当前 SQLCipher provider 中未观察到调用，不能据此宣称全库无风险或继续冻结 3.6.3。AES-CBC 不能与 AES-SIV / CCM 的公告路径混为一谈。

#### 两条路径的影响

| 维度 | 保留 3.6：修补至 3.6.5 | 迁移 4.x：本批限定 4.0.3 |
| --- | --- | --- |
| 兼容范围 | 同系列补丁，预计 API / ABI 与行为变化较小，仍需旧库与恢复验证 | major 版本，必须重编译；不能假定 API / ABI 无变化，也不能直接替换已有二进制 |
| 接入成本 | 没有现成 3.6.5 的 300 系列发布包；需维护真实版本的 openssl-src 源码封装，或独立构建官方 3.6.5 并显式接入 | 已有 openssl-src 发布包；需解决 openssl-sys 的 vendored 版本约束，并验证现有 4.x 分支是否充分 |
| 供应链 / 许可 | 自维护源码封装意味着追溯 OpenSSL 官方归档 / 签名、包装代码、构建参数及实际许可；不能改缓存冒充原包 | 保留发布的 400 系列源码包；若使用本地 openssl-sys 补丁，保存完整来源、原包哈希、最小 diff 与 MIT，OpenSSL Apache-2.0 / 包装许可重新收集 |
| 生命周期 | 2026-11-01 即停止公开支持；投入自建封装后仍很快需要再次迁移 | 4.0 支持至 2027-05-14，窗口更长，但不能作为多年稳定基线冻结 |
| 主要工程风险 | 第二套原生下载 / 编译入口、头文件与静态库混配、notices 仍引用旧 bundle，以及维护短命分支 | 移除旧 API、初始化 / 清理和算法获取行为变化、原生编译与绑定兼容；需要实际 OpenSSL 运行证据 |
| 本项目判断 | 仅当存在迫切修补期限且 4.x 有明确阻塞时采用；必须同时约定退出日期 | 推荐先执行下述有停止线的最小迁移验证，通过后再采用 |

[4.0 官方迁移说明](https://docs.openssl.org/4.0/man7/ossl-guide-migration/)与[移除 API 清单](https://docs.openssl.org/4.0/man7/ossl-removed-api/)明确了 major 兼容变化，包括 ENGINE / 旧接口、ASN1_STRING 不透明化、X509 const 变化、算法获取与退出清理。当前 SQLCipher 的有效 3.x / 4.x provider 路径未使用所列 ASN1 / X509 / ENGINE 接口，因而预计影响小于通用 TLS 应用，这是源码推断而非运行结论。4.0 不再自动注册 atexit 清理；应验证长寿命进程与重复开关库，不为让泄漏报告归零就在单连接关闭时调用全局 OPENSSL_cleanup，以免破坏其他连接。

若目标是多年维护，3.5.9 LTS（官方支持至 2030-04-08）值得另案比较；它是另一个版本方向，不在本次两选一里自动替换，也不默认已解决 Rust 封装与回退兼容问题。

#### 已批准的实施范围

1. 固定 SQLCipher 链及其他包；仅为非 Apple 目标引入 OpenSSL 4.0.3 / openssl-src `400.0.2+4.0.3`。保持现有静态构建、算法 / provider 配置与已启用 feature，不混入 legacy 裁剪、FIPS、TLS、动态库或新密码提供方。
2. 优先采用正式发布、明确支持 400 系列的 openssl-sys；若实施时仍无该版本，使用已批准的**仓库内可审计的 openssl-sys 0.9.117 最小补丁**：保留完整发布源码与许可，仅将 openssl-src 构建依赖约束精确指向 400.0.2，记录原包哈希、补丁 diff 与撤除条件。禁止将 400 包伪装成 300 版本、修改 Cargo 缓存或关闭版本检查。工作树已包含完整 81 文件副本；来源、两个 manifest 的精确版本差异、JSON 末尾 LF 及撤除条件见[补丁说明](../../third-party/openssl-sys-patch.md)。治理检查反向还原差异并逐文件核对原包哈希。
3. 上游可直接替代该补丁且全部验证通过后撤除。后续已单独批准完整 openssl-src 副本和 ARM SVE 前置条件两行修改；除此之外若需修改 OpenSSL C / 汇编、SQLCipher 算法、FFI 绑定、无关包、系统范围或 provider，仍停止并重新评审。
4. 不推荐默认使用独立 OpenSSL 安装目录接入：现有 vendored 路径通过 DEP_OPENSSL_INCLUDE 获取头文件，而普通 bundled-sqlcipher 外部路径会输出动态 crypto 链接。确需独立构建时，必须统一 openssl-sys 与 SQLCipher 的目标头文件 / 静态库、限定 OPENSSL_NO_VENDOR / OPENSSL_STATIC 等目标环境、修正来源清单，并拒绝意外系统 pkg-config / Homebrew 库。它不是设置 OPENSSL_DIR 即告完成的低成本办法。

#### 验证顺序与通过条件

| 顺序 | 必须取得的证据 | 停止条件 / 限制 |
| --- | --- | --- |
| 1. 保存基线与来源 | 从当前 29d8edd / 旧锁定值保存 **SQLCipher 4.19.0 + OpenSSL 3.6.3 的非 Apple worker**、锁定 / 二进制指纹及正常关闭和待恢复 WAL 的合成 A11 库；核对新发布包 registry 哈希、上游来源、许可和构建参数 | 前批 macOS worker 使用 CommonCrypto，不能代替 OpenSSL 旧基线；不从当前 Cargo 缓存手工换源码 |
| 2. 最小构建与实际 provider | 在已授权 Android 环境运行 `cipher_version / cipher_provider / cipher_provider_version`，确认 4.19.0 / openssl / OpenSSL 4.0.3；检查架构、静态符号、NEEDED、compile options 与库体积；Linux 验证宿主另跑相应路径 | macOS 成功和 Android 库构建不能替代 OpenSSL 运行。出现 CommonCrypto / libtomcrypt、动态 libcrypto / libssl 或实际 provider 仍报告旧版本即停止 |
| 3. 算法与数据保真 | 在实际 OpenSSL 目标用公开独立向量覆盖 AES-CBC / HMAC / PBKDF2；验证 RAND 错误传播，创建、错钥、密文篡改拒绝；旧库正常关闭 / 待恢复 WAL 在新版打开，完整摘要、vault / device、候选 / 删除 / outbox、恢复材料、原意图重放均保持 | 不以两个版本自洽代替独立期望；不得重建库、丢候选、重新发身份或调整密码格式来通过；不可因仅验证升级便宣称可降级 |
| 4. 故障、并发与资源 | 复用 pager / 文件发布故障、checkpoint / DELETE、临时文件、SIGKILL / 重启及清理重试；重复开关库和多连接 / 两 bridge 并发，记录 CPU / 内存 / 延迟与旧基线差异 | 并发初始化 / 清理失败、持续资源增长或无法解释的性能回退阻止采用；不以全局 cleanup 破坏其他连接 |
| 5. 工程与目标回归 | Rust fmt / 严格 clippy / C header / 测试；Flutter 两桥接压力 / 错误 / 取消 / 所有权；Android 实际 native harness 与普通 App 路径；iOS 构建及 CommonCrypto 不变的回归；来源 / notices / 公告 / 治理检查 | 新版测试版本断言需同步；现有跨版本用例硬编码 SQLCipher 4.14.0→4.19.0，需补独立 OpenSSL 基线用例，不能拿旧用例结果充数 |
| 6. 采用与恢复边界 | 只提交精确依赖、最小补丁、必要验证与文档；保留旧合成库、版本指纹和失败记录；列明模拟器 / 真机 / Linux 实际执行范围 | 真机保护、认证与系统掉电缺口不自动关闭。未验证新版写库可被旧版读取前，不能将二进制回退称为数据恢复方案 |

Android / Linux 的运行入口、模拟器实例 / 数据与清理范围已在本批实施授权中明确，不能为了补证据私自更换目标、升级系统镜像或修改 CPU 能力覆盖。

#### 首次迁移结果与阻塞记录（72d13b0）

- 来源与最小修改：openssl-src `400.0.2+4.0.3` 发布归档 SHA-256 匹配 registry，2492 个缓存源文件逐字匹配归档；openssl-sys `0.9.117` 的 81 文件来源清单通过。根锁定 135 个包中仅 openssl-src 版本及 openssl-sys 来源改变，133 个包条目和 Pub lockfile 不变。MIT / Apache-2.0 等实际许可已重新收集，notices 检查通过；本批未完成新的 OSV 查询，不能沿用旧查询声称新链无命中。
- Linux 实际 OpenSSL：旧 3.6.3 / 新 4.0.3 worker 都用 SQLCipher 4.19.0；正常关闭和 SIGKILL 后待恢复 WAL 的旧 A11 库由新版成功打开，完整事实、身份、错材料不改写、独立恢复和原意图重放通过。新链 26 项核心 / 桥接 Rust 测试通过，包括 pager、临时文件与并发。现有离线容器是 Rust 1.96.1，旧 / 新使用相同镜像；这项辅助证据不等于主机 / Android 的 Rust 1.96.0 工具链验证。
- 独立算法：显式链接旧 / 新实际静态库的 [C 验证程序](../../scripts/tests/openssl_vectors.c)，以 NIST SP800-38A F.2.5、RFC4231 §4.2、RFC7914 §11 的固定期望检查 AES-256-CBC 加解密、HMAC-SHA512 与 PBKDF2-HMAC-SHA256；正常 RAND 与合成加密库创建通过。独立进程指定不存在的算法提供方后，RAND 失败，SQLCipher 在 sqlite3_open 初始化即报错；没有继续创建明文库。该结果不是随机熵质量证明，也不覆盖 Android 执行。
- Apple 宿主回归：Rust 1.96.0 的 26 项测试、fmt、严格 clippy 与 C header 检查通过；Flutter 19 项宿主测试、格式和严格分析通过。Apple 仍为 CommonCrypto；本批因 Android 阻塞未继续 iOS native 构建或设备回归。
- Android 阻塞：既有 Pixel_9_Pro / API35 / arm64 / 16 KiB 页模拟器上，原生 pager 测试和实际 Flutter App 均在加载库时 SIGILL。addr2line / 反汇编定位到 OPENSSL_cpuid_setup → _armv8_sve_get_vl_bytes 的 cntb 指令。独立 C 诊断读到 HWCAP 的 SVE=0、HWCAP2 的 SVE2=1，PR_SVE_GET_VL 返回 -1 / EINVAL；同设备加载已保存旧 3.6.3 库成功，新 4.0.3 库退出 132。证据支持“当前模拟器能力报告与执行能力不一致触发新初始化路径”，不能推断所有 Android 真机均失败，也不能认定是 SQLCipher 账务错误。

首次仅有 sys 版本补丁时不能通过目标运行门槛，因此当时停止采用、保存候选并继续诊断。此后另行批准的源码接入和复验见下文；仍禁止用 OPENSSL_armcap 覆盖、全局 no-asm、改提供方或跳过失败目标制造通过。

本批证据保存在忽略的 `local-evidence/m0-c/2026-10-25-openssl/`，包含旧基线、来源哈希、日志、加载对照和清理记录。候选按新指令保存提交，未推送；实际下一步以[当前进度](../status/current.md)为准。


#### ARM 能力修复的隔离验证与正式接入方案

候选保存提交为 `72d13b0`。随后在忽略目录从该提交导出源码，并复制已核验的 openssl-src 发布包，仅修改 `openssl/crypto/armcap.c` 一处条件：[两行候选补丁](../../third-party/openssl-armcap-sve.patch)要求先有 ARMV8_SVE，再接受 HWCAP2_SVE2。[来源与前后哈希](../../third-party/openssl-armcap-sve.json)现已更新为正式应用状态；此处记录的是 `1d000cf` 的隔离诊断过程，当时主工作区尚未应用，正式工作区结果另列。

[Linux HWCAP 规范](https://docs.kernel.org/arch/arm64/elf_hwcaps.html)将 SVE2 定义为以 SVE 为前提的能力；[SVE 系统接口说明](https://docs.kernel.org/arch/arm64/sve.html)要求检查 HWCAP_SVE。该修改保留正常能力选择，只拒绝 SVE 缺失而 SVE2 单独出现的矛盾组合，不改算法、KDF、静态链接、provider 或全局环境变量。

在原 Pixel_9_Pro / API35 / arm64 / 16 KiB 模拟器上完成以下诊断验证：

- [能力诊断程序](../../scripts/tests/openssl_armcap_probe.c)分别编译实际上游 / 候选 armcap.c，以合成 auxv、固定期望表和探测调用计数运行 00 / 01 / 10 / 11 四种组合。上游仅在 01 矛盾组合违反期望，候选四组全部满足期望；有效 SVE2 仍执行探测，NEON / AES / PMULL / SHA1 / SHA256 位保持。该单元诊断不等于真实 SVE2 硬件运行。
- 不设置能力覆盖的真实 OpenSSL 4.0.3 provider、公开 AES / HMAC / PBKDF2 向量、正常 RAND 与 SQLCipher 创建、缺失算法提供方的失败传播全部通过。
- 原生 pager 81 组故障 / 3 组对照、临时文件与 MEMORY 策略两组检查通过并清理。普通 Flutter App 的两 bridge 共 16 组恢复、48 组文件故障通过，A11 完整事实 / 身份保留，ownedRuns 为空。
- 双桥接实际 UI 的错误口令、取消晚结果、原库重试通过；结果截图复核输入为空、无截断，系统认证缺口仍显式显示。两 bridge 各 6200 次有界压力调用、并发、panic / ABI / native 错误及取消通过，thin liveBuffers=0；记录 RSS 采样但没有据此宣称长时泄漏或与旧版性能等价。
- iOS arm64 / iPhoneOS native 构建通过，CommonCrypto 符号仍存在，无 OpenSSL 依赖；未执行 iOS 模拟器或真机回归。未修改模拟器配置、CPU 覆盖或系统保护。

现有 openssl-src Build 只从包内源码复制构建，没有可传入补丁 / 替代源码的正式参数。正式接入建议保持现有构建链，增加精确锁定的本地 openssl-src 源包副本、根 patch 和逐文件来源核对，再应用上述一处语义改动；不引入新的 wrapper crate、下载器、编译器拦截或修改 Cargo 缓存。代价为 **2492 个文件、约 33.16 MB 原始源码**（最大单文件约 1.37 MB），以及后续上游修复的跟踪 / 撤除责任。原包另有 31 个文件带末尾空白或缺少末尾 LF，若保留原始字节，需要明确的、仅在来源哈希验证后生效的第三方文本例外；不能泛化关闭仓库卫生检查。

这一较先前 sys 81 文件副本更大的来源 / 治理范围，已获项目所有者明确批准。正式接入保留完整包、逐文件哈希和原模式，应用既定补丁并补齐构建资产依赖。31 个文本例外与 77 个 Git 空白路径按固定清单核验；后者额外覆盖空白末行、空格后接 Tab 等 Git 规则，不是新增目录通配豁免。未引入另一套构建入口、改动缓存或更新模拟器。

诊断证据位于 `local-evidence/m0-c/2026-10-25-openssl-armcap/`。测试 App 已由 Flutter driver 卸载，包不存在；本批模拟器与 ADB 已停止。隔离构建临时复用的本地 debug 签名路径链接已移除，没有复制密钥。该隔离批次当时未将 33 MB 源包加入主工作区；后续正式接入由独立授权和验证批次完成。

#### 正式工作区接入结果

项目所有者批准后，在 dev / 1d000cf 的干净工作区加入完整源包及精确来源清单。2492 个文件中只有 armcap.c 与原包不同，112 个可执行模式保留；两份本地 source 包不加入 workspace 成员。根 lockfile 仍为 135 个包，仅 openssl-src 的 registry 身份变为受校验的本地来源，其他 134 条与 Pub lockfile 不变。Flutter hook 的真实输出已登记两份源包输入，包括 armcap.c；native notices 补齐本地来源和 Text::Template 嵌套许可。来源漂移、例外扩大、模式 / patch 改动与嵌套许可缺失的拒绝回归已通过。

正式批次 `local-evidence/m0-c/2026-10-25-openssl-adoption/` 取得以下证据：

- macOS Rust 1.96.0：26 项测试、fmt、严格 clippy 和 C header；Flutter 19 项宿主测试、格式及严格分析通过。iOS arm64 / iPhoneOS 构建通过，CommonCrypto 符号保留，无 OpenSSL 调用依赖；此批未执行 iOS 设备回归。
- Linux arm64：既有离线 Rust 1.96.1 镜像内 26 项测试通过；另显式执行旧 OpenSSL 3.6.3 worker 的正常关闭 / SIGKILL 待恢复 WAL 两状态升级，A11 完整事实 / 身份、错材料不改写、独立恢复与原意图重放通过。该辅助镜像不代替项目固定 1.96.0 工具链。
- Android 原失败模拟器：实际 SQLCipher 4.19.0 / SQLite 3.53.4 / OpenSSL 4.0.3、独立 AES-CBC / HMAC / PBKDF2 向量、正常 RAND 与缺失提供方失败传播通过；四组合源码诊断保留有效 SVE2 路径。正式 APK 的 native 库为 7372912 字节，NEEDED 无 libcrypto / libssl，不使用能力覆盖。
- Android 原生 pager 81 组故障、3 组对照和 2 组临时文件检查通过；普通 App 两桥接共 16 组恢复、48 组文件故障通过并清理。实际 UI 错误输入、取消晚结果及原库重试通过，截图复核输入为空、布局完整；各 6200 次压力、并发和取消通过，薄 ABI 活跃 buffer 为 0。
- 有界资源记录：同一 Linux 镜像各五次公开向量加建库，旧 / 新耗时中位数为 92 / 90 ms；仅此工作负载未见明显回退。Android thin RSS 四次约 325–335 MB，FRB 约 378–382 MB，取消各 1 次核心调用、40 检查点且晚结果隐藏；不据此宣称长期泄漏排除或真机性能等价。
- OSV 对当前 131 个 registry 包加 2 个本地包的上游名称 / 版本查询，共 133 项无命中；本地补丁完整性另由源文件与补丁哈希核验，OSV 不审计本地差异。40 个 Pub 包沿用前批记录，不冒充本次复查。

Linux 首次因证据输出目录尚未建立而失败；创建指定目录后完整重跑通过。Android 独立 provider harness 首次默认使用无权限临时目录，改为自身 App 私有 UUID 目录后通过；没有放宽生产权限或测试断言。真实失败与最终结果均保留。测试 App、模拟器、ADB、容器及本批合成数据已清理。Android 系统认证仍为 deviceSecure=false / 代码 11，真实 SVE2 硬件、真机 release / 保护与掉电仍未验收；本次完成依赖接入，不宣称 M0 退出或数据库可降级。

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

普通 `main.dart` 只运行 `ProbeApp`；`file_boundary_main.dart`、`bootstrap_main.dart` 和 `recovery_main.dart` 是显式构建的合成入口。当前实验库仍包含 validation ABI、平台 MethodChannel 和合成故障能力，不能把“普通页面未调用”描述为生产包已编译排除。它们不构成外部导出命令服务；进入生产打包前须另验产物入口、导出能力、日志及诊断资产的隔离。

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

`store_pager_tests.rs` 的故障调度仅由 `cfg(all(test, unix))` 接入，可以单独编译为移动模拟器原生测试可执行文件。临时工作负载与 VFS 透传器由 `store_boundary.rs` 复用，供第 7.11 节的专用合成 App 入口调用；App 不开放 pager 故障调度，也不切换默认 VFS。父测试在独占子进程注册观察 VFS，原 VFS 继续执行真实文件 IO、锁、SHM 与 mmap；父进程及其他并行测试不切换默认 VFS。随机合成 DB key 仅通过 stdin 传递，不写参数、环境变量、报告或日志。

先取得同一 A11 账库在 WAL 提交、DELETE 日志提交和 checkpoint 中的成功 IO 序列，再逐个写入 / 同步位置注入；每次核对文件类别、调用序号、长度 / flags 和偏移，未命中不能通过。SQLite 的 `xWrite` 返回错误码而非已写字节数，所以短写场景实际调用原 VFS 写入严格少于请求的 2–7 bytes，再返回 `SQLITE_IOERR_WRITE`；不把正数当作 SQLite 的成功返回。`xSync` 分别在调用父同步前拒绝，以及父同步成功后报告 `SQLITE_IOERR_FSYNC`，覆盖可能已经持久化但调用方收到失败的情况。此处验证的是 pager 面对 IO 失败的行为，不证明内核实际发生短写 / fsync 故障或掉电。

每次操作返回后保持连接打开，父进程发送 SIGKILL 并等待实际退出；随后父进程通过普通 `Store::open` 恢复。A11 候选、删除和 outbox 必须保持，新增 123 分转账只能完整存在或完全不存在：余额为旧的 10000 / 8000 分或新的 9877 / 8123 分，净资产均为 18000 分。核对人工计数、完整摘要、审计及 `integrity_check` 后，用相同操作身份重试并再次重放；不得重复入队。checkpoint 失败必须保留此前已提交的完整新事实。同步报错不能推定事务没有提交。

临时文件验证使用 8192 行合成数据和受限页面缓存，按下表建立实际工作负载与观测。`xOpen` / `xWrite` 记录文件类别和阶段；`xClose` 前通过原 VFS 分块扫描仍打开的真实文件，包含已经 unlink、目录枚举看不到的匿名文件。单文件扫描上限 64 MiB，超界或读取失败均阻断；不截断后宣称通过。SHM 由 `xShmMap` 单独计数，并在连接仍打开时扫描实际 sidecar。

| 文件 / 操作类别 | 判据与正向对照 |
| --- | --- |
| DB、WAL、回滚日志、SHM | 真实文件 / 映射存在；加密构建的合成明文标记不应出现在实际写入与文件扫描中 |
| TEMP 表、排序、自动索引、物化查询、索引创建 | 原 `temp_store=MEMORY` 不打开磁盘临时文件；允许磁盘临时存储的构建中，隔离 FILE 对照必须在每一阶段实际落盘；`TEMP_STORE=3` 构建则要求 FILE 请求仍不落盘，并明确记录无磁盘临时文件正向对照。另核对排序结果、查询计划、排序计数、自动索引行数与物化结果 |
| 语句回滚 / sub-journal | 大批更新最后触发 UNIQUE / ABORT，先前更新全部撤销；允许磁盘临时存储时，FILE 对照必须观察非空 sub-journal，不能只验证最终表内容 |
| VACUUM | 验证重建与 A11 保全；允许磁盘临时存储时，FILE 对照观察真实临时 DB，逐个记录是否包含标记，不假设所有临时文件都明文或都加密 |
| 多库提交 / super-journal | 仅在本次合成目录附加独立加密库，DELETE 模式下同时提交两库，观察非空 super-journal 与清理；这不是生产 ATTACH / 多库事务能力声明 |

`TEMP_STORE=2` 构建的 FILE 对照会在 TEMP 表、排序或索引等临时文件检出合成明文，这是用于验证观测有效性的预期结果；不能据此放宽生产 MEMORY 策略。SQLite 对临时文件类别与生命周期的说明见[临时文件](https://www.sqlite.org/tempfiles.html)、[VFS 契约](https://www.sqlite.org/c3ref/vfs.html)及 [IO 方法](https://www.sqlite.org/c3ref/io_methods.html)；SQLCipher 的临时存储保护边界见[官方设计](https://www.zetetic.net/sqlcipher/design/)。测试实现为本项目编写，仅使用已锁定绑定，不复制第三方 VFS 示例或新增依赖。

日常入口为 `check-core.sh`。定向运行：

```sh
CARGO_NET_OFFLINE=true cargo test --locked -p radishledger-core --lib store::pager_tests -- --nocapture
```

所有平台另用独立、未加密的合成数据库验证 `xRead` 扫描能实际检出标记；该对照在加密文件 / 临时文件观测截取之后执行，单独记录，不混入 MEMORY 策略判据。Android 编译时强制内存的结果不能当作匿名磁盘临时文件扫描的正向证据。

可将 `M0_PAGER_EVIDENCE_DIR` 指向已创建的 `local-evidence/m0-c/<batch>`，保留不含 key / 页面载荷的 JSON；解析后的目录不能越出项目 M0-C 证据范围。两个父测试实际执行全部子进程，标为 ignored 的 `pager_worker` 仅是专用子进程入口，不是跳过故障覆盖。每个子进程最多等待 45 秒；超时 / 失败只终止本次 PID 并等待回收。宿主入口的 `TMPDIR` / `SQLITE_TMPDIR` 仅在子进程指向本次独占目录；移动入口还将父测试限制在本轮 App 私有目录，正常完成先关闭 / 回收进程与文件，再清理目录并保存成功报告。

该切片覆盖有界工作负载中的 SQLite 临时文件类别；宿主与下节的移动原生模拟器分别取得证据，不等于所有查询形态、生产 App 安全域、真机、长时缓存、OS swap / crash dump、导出或归档边界已经验证。现有产品未开放任意 SQL、ATTACH 或导出入口；新增路径需补相应文件审计，不能直接沿用本批通过结论。


### 7.10 移动模拟器原生 pager 复验

复用上节同一 `cfg(test)` harness 与正式 Store 实现，`build-mobile-core.py --pager-tests` 沿用普通 code asset 的 Rust 版本、NDK / Apple SDK、目标和最低系统参数，生成唯一的 release 原生测试可执行文件；不新增依赖、测试 feature、桥接接口或 App 故障开关。构建输出与 App code asset 分开，`--frb` 与该模式不并用。

先按已有授权启动确切的现有模拟器并安装本项目普通入口 debug App，再执行下列入口。以下路径用于本地复现；工具链和设备必须先核对，不通过命令自动安装或重建模拟器：

```sh
CARGO_NET_OFFLINE=true python3 scripts/build-mobile-core.py \
  --os android --arch arm64 --minimum-version 24 \
  --c-compiler /Users/luobo/Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang \
  --pager-tests --output local-evidence/m0-c/<batch>/android-pager-tests
CARGO_NET_OFFLINE=true python3 scripts/build-mobile-core.py \
  --os ios --arch arm64 --ios-sdk iphonesimulator --minimum-version 15.0 \
  --pager-tests --output local-evidence/m0-c/<batch>/ios-pager-tests
python3 scripts/check-mobile-pager.py --platform android --device emulator-5568 \
  --binary local-evidence/m0-c/<batch>/android-pager-tests \
  --output-dir local-evidence/m0-c/<batch>/verified
python3 scripts/check-mobile-pager.py --platform ios \
  --device CB767BAB-1B1B-4AF8-A3FB-0C2B793A0756 \
  --binary local-evidence/m0-c/<batch>/ios-pager-tests \
  --output-dir local-evidence/m0-c/<batch>/verified
```

控制器复用 `mobile_probe_device.py` 定位 App，只接受现有模拟器。Android 通过 `run-as` 在本项目 App UID 下执行，实际安全域为 `runas_app`；iOS 通过 `simctl spawn` 运行目标可执行文件，使用本项目数据容器。它们证明移动目标的原生核心、VFS 和该文件路径上的行为，不证明 Flutter bridge 调用、生产 App sandbox / entitlement、系统认证、真机或实际掉电。App 生产入口不被替换。

每轮先保存实际 `target_os` / ABI、默认 VFS 名与版本、SQLCipher / SQLite 和全部 compile options，拒绝宿主目标或其他 ABI；每个故障与临时文件子进程的配置必须与预检完全一致。可用 `--configuration-only` 单独预检。测试报告中的原 VFS 均在注册观察器前读取。Android 另记录 App UID、SELinux 域、API 与系统页大小；iOS 记录实际 Simulator runtime。

原生测试程序、`TMPDIR` / `SQLITE_TMPDIR`、随机合成库和报告仅位于本项目 `code_cache/m0-pager-<UUID>`（Android）或 `tmp/m0-pager-<UUID>`（iOS）。移动 `M0_PAGER_EVIDENCE_DIR` 只允许该 TMPDIR 下的 `evidence`。随机 key 仍由父子进程 stdin 传递。正常返回先核对所有原始测试目录已清理，再复制非秘密 JSON 到宿主忽略证据目录，最后精确删除本次 UUID 目录；不清理其他缓存或其他 App。每个报告保存测试二进制 SHA-256、目标、实际路径、退出码和清理状态，不覆盖已有批次。

若失败，保留日志和已登记的精确目录，先排查失败阶段及本次进程，不得把 `cleaned=false` 改记通过或按宽泛前缀删除。完成后核对本轮控制器 / 测试进程与目录无残留，停止本轮启动的模拟器；既有 adb 或其他设备进程保留。锁屏、设备定义、快照与系统保护项不由此入口改动。最终结果与残余门槛维护在 current，历史批次记录在 devlog。


### 7.11 App 进程安全域与文件路径复验

本入口补充第 7.10 节未覆盖的正常 App 进程、桥接和文件路径。复用已有 M0 App、平台 `storage` 策略与两种 bridge 的 validation 调用；普通 `main.dart` 不执行本实验。它仍是合成验证 App，不是最终用户产品或正式发布包。

真机准备时先读[现有实现与入口缺口](mobile-core-validation.md#63-现有实现与真机入口缺口)和[单批操作单](mobile-core-validation.md#64-单批操作单与验收停止线)。本节控制器仍只支持模拟器；可选的 iOS 有界锁屏观测见第 7.15 节，不能把真机属性读取等同于实际文件保护验收。

`file_boundary_main.dart` 通过 `M0_APP_BOUNDARY_RUN` 接受 32 位小写十六进制批次身份。核心 `fileBoundary` 只在既有私有根下排他建立 `m0c-<run>`，随机 DB key 仅存在内存，不接受真实账库 / 解锁材料。运行 A11 后复用第 7.9 节的 8192 行临时文件工作负载；每个连接显式选择唯一命名 VFS，注册 / 注销均不改进程默认 VFS，另以普通 Store 连接验证观察器未截获默认连接。连接的 key / PRAGMA 配置仍共用 Store 原实现，不建立第二套存储策略。

每端依次运行薄 FFI / FRB × MEMORY / FILE 请求四组，独立核对进程 ID、完整编译选项、完整账务摘要、8 个修订 / 序号、7 个 outbox、2 个候选、1 个删除，以及 10000 / 8000 分余额。所有 VFS 文件必须成功扫描并关闭。留存真实非空 WAL / SHM 供平台读取属性，再由 `fileBoundaryCleanup` 校验归属和锁后删除本次目录。

| 观测 | 判据与限制 |
| --- | --- |
| Android 进程 | 正常 Activity 启动的 release 包，运行时 `debuggable=false`、`allowBackup=false`，`/proc/self/attr/current` 为 `untrusted_app`；外部 `run-as` 与 shell 私有目录访问实际拒绝 |
| Android 路径 | 必须等于 `noBackupFilesDir/m0-c`；根 / 子目录 0700，DB、WAL、SHM、辅助库、扫描对照和 owner / lock 均 0600、App UID，实际 SELinux 标签为 `app_data_file` |
| iOS 路径 | 必须等于本 App 容器 `Library/Application Support/m0-c`；根备份排除标记为 true，目录 0700、上述文件 0600、同一有效 UID；Simulator 不证明真机 sandbox 隔离 |
| iOS 文件保护 | 原 storage 仍请求 `NSFileProtectionComplete`。可读取该属性时记录为属性观测，硬件保护仍未验证；仅 Simulator 返回 null 可标为 `unavailableOnSimulator`，不能改记保护通过。真机缺失、任意较弱属性均拒绝 |
| VFS 短命文件 | 实际 DB / WAL / 回滚日志在本次目录且为 0600；super-journal 使用 SQLite 的 0644 默认值再受进程 umask 限制，允许实测 0600 / 0644，父目录必须 0700。匿名临时文件按原 VFS 关闭前扫描，不虚构可枚举路径 |
| 正向对照 | Android `TEMP_STORE=3` 的 FILE 请求仍无磁盘临时文件；iOS `TEMP_STORE=2` 的 FILE 请求必须实际产生四类临时文件并检出匿名文件标记。独立明文 scanner 对照与加密文件判据分开 |

App 在调用前原子登记批次 / 目录 / bridge / run / PID；只含合成身份。控制器可在 `filesReady` 实际停止 App，重启后验证新 PID、精确回收登记目录并完成全部四组。中断恢复证明本实验材料可清理，不是随机内存 key 丢失后可恢复账务；后者仍由 bootstrap 实验验证。登记损坏或归属不明时保留现场、明确失败，禁止按目录前缀批量删除。

使用现有工具链构建专用入口，Android 必须 release，iOS Simulator 使用 debug；设备 / 包安装授权和精确恢复计划按项目协作规则执行。构建与 Flutter 检查串行使用同一工作区，沿用锁定依赖。

```sh
# 在 apps/m0_probe；批次值是新生成的非秘密身份，不是 key。
flutter build apk --release --target-platform android-arm64 --target lib/file_boundary_main.dart --dart-define=M0_APP_BOUNDARY_RUN=<session>
flutter run --debug --no-resident -d <existing-ios-simulator> --target lib/file_boundary_main.dart --dart-define=M0_APP_BOUNDARY_RUN=<session>
# 回到仓库根；App 已安装且设备已启动。
python3 scripts/check-mobile-app-files.py --platform android --device <existing-emulator> --session <session> --output-dir local-evidence/m0-c/<batch> --interrupt-once
python3 scripts/check-mobile-app-files.py --platform ios --device <existing-ios-simulator> --session <session> --output-dir local-evidence/m0-c/<batch> --interrupt-once
```

`flutter run` 会自动启动一次实验；控制器前需核对并保存该次结果、确认已清理，再移除该精确报告。控制器拒绝混入旧 iOS 报告或覆盖既有输出。Android 不开放调试 / 导出组件，专用入口以 700 字符分片发布不超过 96 KiB 的合成诊断；接收端核对分片长度、数量与完整 JSON，截断不能当作通过。iOS 从本 App 临时目录读取合成报告；读取未完成只在整体超时内等待，不改变最终判据。报告不含 DB key、页面载荷或真实账单；普通入口不输出这些诊断。

收尾核对全部已登记目录、报告和 journal 清理；截图需确认最终状态而非运行中画面。按初始状态恢复普通入口或移除本次临时安装包，停止本轮 App / 模拟器，保留原有 adb。原生 fault harness 结果不能与本 App 的四组无注入观测混算。本切片不证明真实系统认证、iOS 真机 entitlement / 文件保护、备份传输行为、最低系统、任意查询 / 导出缓存、真实 IO 故障或掉电。


### 7.12 异常释放与清理重试边界

命名 VFS 的连接必须先于注册对象释放，包括打开 / 密钥配置失败和验收断言展开。`FileObservation.closed` 只表示底层 `xClose` 返回成功；`close_code` 为 `null` 表示尚未完成关闭，为整数表示关闭回调已结束并记录原返回码。shim 在回调结束后释放文件内存，注册对象按回调完成状态注销；不能因为底层报错，在失败展开期间再次 panic。验收仍要求 `closed=true` 和扫描成功，不把资源已释放当作 IO 成功。

宿主回归分别覆盖：真实空文件关闭后注入 `SQLITE_IOERR_CLOSE`、验收失败仍可展开 / 注销 / 再开普通连接；错误密钥导致 `Store::configure` 返回错误后命名 VFS 注销、默认 VFS 不变且原库可用正确密钥读取。关闭错误为回调注入，不是内核实际 close 失败；移动目标仍需在下一次相关设备批次补验。

`Files::cleanup` 先验证平面目录中的所有条目，再删除载荷，最后撤销 owner、移除 lock 和目录。载荷删除失败时保留原 owner / lock 并返回 `bootstrap_io`，后续可用同一身份重新取得锁并精确清理。删除回调注入只用于宿主回归，不增加对外故障字段；最终 owner / lock / 目录移除或父目录同步失败仍保留为错误与人工核对边界，不宣称任意 IO 故障、并发目录篡改或掉电均能自动恢复。

专用 App 的登记 / 诊断仍使用平台临时目录，只证明已观测的跨进程中断恢复；不能推定 OS 清缓存、重装或设备重启后登记一定存在。真实备份、重装与归档恢复另按两端矩阵验证。


### 7.13 人工实验页的精确失效、清理与候选切换

`bootstrap_main.dart` 的默认人工页增加三条操作：使本次活动日常保护项缺失、使用恢复材料重建日常保护、清理本次合成实验。仍复用 `BootstrapService`、平台精确 `delete` 与 Rust bootstrap；不改变密码格式 / 提供方，不开放外部控制组件。`M0C_DRIVER=true` 的模拟器 FIFO 路径和设备限制保持原样。

- **精确项缺失：** 先读当前活动 generation，弹窗显示 run / generation 和影响；确认后再读一次活动身份，不一致则拒绝。仅删除该平台项，保留活动封装、原账库、独立材料与未同步事实，供系统解锁失败、独立恢复及重建实验使用。它不等于真实生物识别 / 锁屏设置失效。弹窗期间切后台会撤销此次等待；已执行的删除不能靠取消撤回。
- **独立重建：** 新按钮直接使用恢复材料调用原 `establishProtection('recovery', …)`；仍由核心先验证原账库完整事实，再建立、读回及提交保护项。成功报告仅保留平台返回的 `securityLevelCode` / `protection`，硬件验证标志始终为 false；其他平台字段不进入报告。Android API 30 的 `unmeasured` 仍保持未知，API 31+ 原始代码也不等于已经实测硬件安全等级。
- **清理与切换：** 首次清理弹窗明确将删除本次合成账务、未同步操作、候选、独立恢复封装与系统保护项。只有本次平台候选和核心目录均清理后才撤销 selection，人工页换新 run 并重新允许选择 bridge；不先卸载 App，也不按前缀批量删除。失败后保留登记，页面只允许重试清理，不再解锁、重建保护或创建新账库；重试沿用已确认范围。

[BootstrapSelection](../../apps/m0_probe/lib/src/bootstrap_selection.dart) 管理本 App 私有根内 `selection.json`，兼容原 `{run, bridge}` 记录。清理前增加非秘密 `cleanupCandidates`：来自同一核心 `status` 的全部 local generation，包含活动项、旧项与认证取消后遗留的候选；最多 256 个唯一的 32 位小写十六进制身份，记录上限 16 KiB。活动身份必须在候选集中，归属必须同时匹配 run / bridge。未知键、坏 JSON、重复 / 非法身份、符号链接和未完成的 `.pending` 发布均拒绝，不能当成“没有账库”。普通创建的空 / 超限口令在写入 selection 前拒绝，核心仍执行正式输入校验。

清理顺序为：完整写入并 flush `.pending` → rename 发布候选清单 → 逐个精确删除平台项 → 原 Rust `cleanup` 校验 owner / lock 并删除本次核心目录 → 核对目录不存在和登记未变化 → 删除 selection。平台项已缺失可以精确重试；若进程在核心目录删除后、selection 删除前中断，后续从已保存清单重新删除平台项并确认核心目录不存在，不再依赖已删除的 bootstrap `status`。平台删除失败、后台中断或核心清理失败均不能提前清掉登记。尚无清理清单而 status 不可读时停止，不猜测候选；核心 owner / lock 最后阶段失败仍沿第 7.12 节保留人工核对边界。

该登记位于 App 私有根而非临时目录；发布采用文件 flush / rename，但没有新增目录同步或掉电保证。遗留 `.pending` 不自动覆盖、丢弃或当作已确认计划；需按精确身份核对现场。它只支持本机人工合成实验的归属恢复，不是生产数据删除协议、备份归档或跨设备恢复格式。

宿主验证入口为 `flutter test --no-pub test/bootstrap_experiment_test.dart test/bootstrap_widget_test.dart test/bootstrap_platform_test.dart`（在 `apps/m0_probe`）。两 bridge 使用实际 Rust / SQLCipher 检查 A11 完整事实和身份、错材料保全、精确项缺失后的独立恢复、重建与清理重试；系统认证与密钥存储为内存替身。widget 测试覆盖确认取消、后台撤销等待、核心清理失败后重开页面及清理后切换 bridge。宿主对象 / 页面重建不称为设备新 PID 或设备重启；实际 UI 视觉、系统认证、两端 release 安装与文件保护仍需对应目标证据。


### 7.14 人工实验的有界非秘密报告

人工页的“查看本次合成诊断”只查看当前页面实例保留的观测。默认不写文件、不输出诊断分片；打开诊断前清除输入框和已展示的恢复材料。只有构建时设置有效的 `--dart-define=M0C_EVIDENCE_SESSION=<32位小写十六进制批次>`，弹窗才允许显式“输出本次合成报告”。缺失 / 非法批次仍可查看，但原值不进入报告，核心操作不受影响。该参数不启用 FIFO、调试权限、设备连接、分享或网络上传。

[bootstrap_evidence.dart](../../apps/m0_probe/lib/src/bootstrap_evidence.dart) 是报告的字段投影来源，报告类型的构造器私有，不能将任意 Map 作为输出快照：

| 字段范围 | 保留与限制 |
| --- | --- |
| 报告身份 | schema 1、批次、每次打开弹窗新生成的 reportId、实际 App PID / OS 与 Flutter build mode；Git、包指纹、签名和设备型号仍需在单批操作单外部记录 |
| 能力 | 仅 supported / deviceSecure 布尔、API 整数、已知 environment 与受限平台错误码；缺少 environment 就是未知，不能从 Android OS 名推定真机 |
| 操作结果 | 固定动作名、run / bridge、进程内递增序号和总耗时；inProgress / succeeded / failed / unknown / cancelledBeforeAction 分开，waitStopped 单独记录。停止等待后收到成功保留 succeeded + waitStopped=true，不宣称核心未提交 |
| 核心摘要 | 格式校验后的合成 vault / device / generation、完整内容摘要、ready / fixtureComplete / cleaned 等布尔、修订 / outbox / 候选 / 删除计数与原始安全等级代码；不包含完整 audit、流水、金额、说明或操作载荷 |
| 失败 | 固定错误代码集合与受限整数 / 已知平台异常类名；未知错误分类记 unrecognized_error，非结构化异常记 unknown，不输出原异常字符串、路径、平台 message 或 details |
| 证据边界 | hardwareVerified 恒为 false，acceptance 恒为 not-assessed；耗时包含整次 UI 调用，不代替 KDF 专项测量。每实例最多保留最近 64 条操作，omittedObservations 显式记录省略量；重开页面 / 进程不恢复旧报告，不保证崩溃前观测已保存 |

报告在弹窗打开时成为不可变快照。输出失败明确提示，可重试同一快照；重新打开弹窗生成新 reportId，不能把两份快照混成一份。每份 UTF-8 报告上限 96 KiB，[共用分片入口](../../apps/m0_probe/lib/src/probe_report.dart)沿用第 7.11 节的 700 字符 base64 分片和有界间隔。人工页标记为 `RLM0_BOOTSTRAP:<session>:<reportId>:<index>:<count>:<data>`；原 `RLM0_APP_BOUNDARY` 格式不变。传输前校验全部分片，不为超限报告静默截断。

在已确认设备 / 安装 / 日志范围后，仅捕获指定 App PID 和批次的合成标记到本地证据目录，不采集整机日志。当前没有新增真机连接或日志抓取控制器；Android / iOS release 日志是否可取、是否丢片及跨 PID 身份仍需实际目标验证。缺少设备时不能把下面的离线重组命令当作设备采集已完成：

```sh
# 仓库根；输入必须是此前明确范围内保存的本批合成标记文件。
python3 -B scripts/collect-bootstrap-evidence.py \
  --input local-evidence/m0-c/<batch>/bootstrap-filtered.log \
  --output local-evidence/m0-c/<batch>/bootstrap-report.json \
  --session <session> --report-id <report-id>
```

[collect-bootstrap-evidence.py](../../scripts/collect-bootstrap-evidence.py) 只接受本仓库 M0-C 忽略证据目录内已保存的普通 UTF-8 文件（上限 2 MiB），不启动任何设备命令。乱序与相同分片重试可以重组；目标标记畸形、缺片、冲突、超限、坏 base64 / JSON、重复 JSON 键、身份 / 版本或固定证据边界不符均失败，不覆盖既有输出。错误不回显原始输入；IO 错误保留 errno。输出成功只说明传输与外层报告边界满足要求，不验证 App 来源、不认证内容、不清洗任意输入为公开材料，也不根据 succeeded / 摘要 / 计数判断 B / C 验收。原始报告和过滤日志仍留在本地忽略目录，不随源码提交。

`bootstrap_evidence_test.dart` 验证字段排除、未知失败、停止等待与晚成功、记录上限、默认禁止输出、传输失败重试、原文件报告格式，以及 Dart 实际输出到 Python 收集器的往返；`bootstrap_experiment_test.dart` 另以两 bridge 的真实核心结果核对摘要投影。`scripts/tests/test_bootstrap_evidence.py` 验证缺片 / 冲突 / 畸形 / 大小 / 身份 / 路径 / 非普通文件 / 输出不覆盖。上述均为宿主证据；报告不是完整审计归档、设备签名证明或真机结论。


### 7.15 iOS 有界锁屏访问观测

在第 7.11 节专用入口额外设置 `--dart-define=M0_IOS_LOCK_OBSERVATION=true`，可在第一组 thin / MEMORY 的真实合成文件准备与属性核对后，进入 45 秒人工锁屏观测窗口。默认不开启，Android 拒绝该选项，Simulator 原生入口直接返回 `unsupportedSimulator`。此选项不授权安装、签名、锁屏设置变更或设备操作；实际批次仍先填操作单。

[M0FileProtectionProbe](../../apps/m0_probe/ios/Runner/M0FileProtectionProbe.swift) 校验随机 run、精确 owner、同 UID 的普通 DB / WAL / SHM，并以不跟随链接的目录描述符固定归属。每次只重新打开三个文件、核对 inode / device、读取一个字节并关闭，再以 `O_WRONLY` 打开并关闭；对这三个文件不带 create / truncate、不写字节、不输出文件内容。写方式打开不等于实际 write、SQLite 写事务或已有 mmap / 数据库文件描述符的锁屏行为。

实际写入使用同一已归属目录中的独立 `lock-write-control`：`O_CREAT | O_EXCL | O_NOFOLLOW` 排他创建、0600、同 UID，iOS 必须实测继承的 `NSFileProtectionComplete`，属性缺失或较弱即失败，不现场修改属性制造通过。已有对照文件拒绝覆盖或复用，失败保留它供原 owner 清理。仅这个无账务内容的描述符保留到观测结束；各阶段真实调用 `pwrite(fd, ..., 1, 0)`，固定字节交替，成功后调用 `fsync` 并 `pread` 回读核对。返回各调用的实际结果 / errno，不输出字节值。这样即使 DB 的 open 已被拒绝，也可区分对照文件的实际 write 是否执行；对照不代表 DB / WAL / SHM 写入、SQLite 提交或耐掉电保证。Complete 与允许已打开文件继续访问的 CompleteUnlessOpen 不混用，语义见 [Apple 文件保护类型](https://developer.apple.com/documentation/foundation/fileprotectiontype)及 [CompleteUnlessOpen](https://developer.apple.com/documentation/foundation/fileprotectiontype/completeunlessopen)。

原生阶段依次为可访问基线、保护数据不可用时的一次访问、恢复可用后的一次访问。每次记录 `isProtectedDataAvailable` 前后状态、文件名、实际 IO 阶段、返回值、errno 和关闭错误；只有状态前后均不可用、三个文件的 read / writeOpen 均得到明确 `EACCES` / `EPERM` 且无关闭错误，才设置 `accessDeniedObserved=true`。文件缺失、链接、身份变化、EOF、普通 IO 错误与状态变化均不能当作保护拒绝。解锁恢复要求同一身份的三个文件全部恢复可读和写方式打开；`accessRestored` 只代表这三个文件的访问恢复。独立 `controlWriteDeniedObserved` 只在状态前后均不可用且实际 pwrite 返回 -1 / EACCES 或 EPERM 时为 true；缺少调用、0 字节短写、EBADF、EIO、状态改变或单纯 open 拒绝均不能通过。`controlWriteRestored` 需真实写入 1 字节、fsync 成功、回读匹配且可用状态稳定；初始基线也要求该对照完整成功。

采用包含系统休眠时间的 `mach_continuous_time` 截止时间和主线程 250 ms timer；不申请后台延长、不增加 entitlement、不持有跨阶段数据库文件描述符。到期检查先于新的 IO：进程恢复时已超过窗口且未执行锁屏调用，返回 `lockedIoNotExecuted`，不能断言确实发生过暂停；已有锁屏调用但未及时观察恢复，返回 `restorationNotObserved`。App 暂停期间无法保证准时回调，只能在获得下一次执行机会时结束。`protectedDataWillBecomeUnavailableNotification` 发生在实际不可用之前，不将通知本身当作拒绝证据；状态含义依据 [Apple 可用状态说明](https://developer.apple.com/documentation/uikit/uiapplication/isprotecteddataavailable)和[保护不可用通知](https://developer.apple.com/documentation/uikit/uiapplication/protecteddatawillbecomeunavailablenotification)。

UI 可取消本次窗口，原生 timer / 引用在正常、取消、基线失败或超时返回时释放，显式关闭对照与目录描述符并记录两个 close errno；Flutter 对资源关闭失败保留失败状态，随后沿用精确目录清理。原 Rust owner 清理同时覆盖新增对照，thin / FRB 宿主检查均验证该文件随本次目录清理。若保护尚不可用导致清理失败，报告失败并保留已有归属登记；解锁后以同一 session 重开原入口核对和重试，不另建前缀清理。登记仍是临时目录文件，不增加掉电保证。

结果置于原文件报告的 `lockObservation`，含精确 run / bridge；`outcome=observed` 仅表示两阶段均执行，不意味着拒绝与恢复成功。外层 `passed` 判断原四组文件实验、观测资源关闭和清理，不能代替锁屏判据。所有结果保留 `hardwareProtectionVerified=false`、`acceptance=not-assessed`。原实验随机 DB key 在核心调用后释放，因此本切片不验证解锁后的 SQL / A11 事实恢复；对照的真实设备 write 拒绝、恢复后原事实保全、跨 PID 与真机 release 日志仍需单独补齐。A11 接入方案见第 7.16 节。

macOS 可运行 `sh scripts/check-ios-file-protection.sh`：现有 Swift 工具链编译真实原生采样器，以临时合成文件验证 DB / WAL / SHM 内容未改写、身份替换与链接拒绝，对照真实 pwrite / fsync / 回读、已有对照保留及描述符关闭；再用注入状态 / 错误验证拒绝分类、短写 / 同步失败基线拒绝、恢复、取消及跨截止时间不补记调用。临时文件与编译缓存在退出时精确清理。此入口不连接设备，不模拟硬件保护；原生目标类型检查与 Flutter 宿主检查也不代替完整 App 构建或移动 UI / 真机执行。


### 7.16 解锁后 A11 前后复核的接入方案（未实现）

第 7.11 节的临时文件实验在调用结束后丢弃随机 DB key，不能在外部保留或公开 key 来补恢复。后续切片复用现有人工 bootstrap 页、`BootstrapService` 与 Rust `Inspect`，使用原独立封装 / 日常保护重新解锁，不新增密码格式、key 缓存、旁路 SQL 或系统保护策略。以下为接入与验证要求，尚不是已具备的 App 流程。

1. **同库基线。** 在当前 selection / bridge 下显式解锁，要求 ready / fixtureComplete，并核对完整 contentDigest、vault / device 身份、8 个修订、7 个 outbox、2 个候选和 1 个删除。随后只保留非秘密基线与本次观测身份；在启动原生观察前清空输入框、已展示恢复材料和可控 secret buffer，不让待执行闭包保留口令 / 恢复文本。
2. **真实文件范围。** 原生观察绑定同一 bootstrap run，先核对 owner、根 / DB 属性及实际存在的 sidecar。正常关闭后的 WAL / SHM 可能不存在，必须分别记为未覆盖；不创建空 sidecar 或沿用另一随机库的文件证据。第 7.15 节当前要求三个文件齐备，不能直接作为该接入已经可用的证明。独立写对照仅是附属合成文件，观察结束不自动清理原 bootstrap 账库。
3. **重新取得解锁能力。** 窗口返回且设备恢复可用后，由用户再次输入独立材料或重新完成系统认证，调用原 Inspect。不能复用锁屏前已解包 key，不能把 native `accessRestored` 当作 SQL 成功。原生观察跨后台等待与 UI 的 `_epoch` 取消语义分别处理：后台仍清除敏感输入，晚结果只能归属原 run / bridge，不能覆盖新选择的界面或报告。
4. **比较和停止。** 重新核对相同 run / bridge / vault / device、完整 contentDigest 与 A11 身份 / 计数；任何差异保留现场并失败，不重新 seed、replay、重建或覆盖原库使摘要一致。实际 IO 未执行、仅属性通过、用户取消、解锁失败和摘要不一致分别报告；重新解锁成功不自动补记锁屏拒绝。
5. **恢复与证据。** 取消只停止本次等待，原账库与已有恢复路径保留；原有确认清理流程负责最终删除。第一切片可只保留本进程非秘密基线，进程退出后标为基线缺失，不能宣称跨 PID 恢复已比较。诊断继续固定字段投影和有界输出，不能塞入完整 audit / 解锁材料。

接入验证先在宿主对 thin / FRB 的真实核心执行前后 Inspect、错材料保全、完整摘要 / 身份比较、取消后重新解锁以及清理重试；状态机与平台拒绝用替身时须明确标注。再验证后台期间输入清除、晚结果隔离、无 sidecar、缺失基线和新 UI。只有相应目标上的实际操作才可补移动 / 真机矩阵，不将本方案或宿主重开对象当作执行证据。
