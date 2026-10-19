# 当前状态

更新日期：2026-10-19（Asia/Shanghai）。

## 当前结论

项目处于 M0 移动核心与加密恢复验证。2026-10-19 已将 A11 非空候选 / 删除 / 未同步 outbox、跨进程恢复、原意图重放及封装文件故障复验扩展到两端现有模拟器的双 bridge；每端 16 组恢复、48 组文件故障及实际 UI 路径通过。Android 本次实测仍无安全锁屏认证条件，系统保护闭环未通过；未修改锁屏设置。宿主另有实际 DB / WAL / 回滚日志检查及两端 native release 编译证据；M0-B / M0-C 的正式退出条件保持不变。

10 月 14 日 M0-C 已按确认范围接入固定密码依赖、随机独立 bootstrap、口令 / 恢复材料及本项目系统保护适配，见[实验专题第 7 节](../development/mobile-core-prototype.md#7-m0-c-bootstrap解锁与独立恢复方案)。两端既有模拟器已取得双 bridge 的跨 PID 解锁、错误材料保全及六处中断证据；iOS Simulator 的精确 Keychain 项删除、独立恢复和重建后重启通过。当批 Android 设备没有认证条件，明确拒绝系统保护，保留设置；Android 受保护日常解锁仍未通过。未宣布 M0-C 退出。

已有 Flutter 合成验证 App、同一 Rust 领域 / SQLCipher 事务核心，以及薄 C ABI / Dart FFI 与 flutter_rust_bridge 两种候选。它们执行同一份人工预期材料，不接收真实账单。没有可供用户使用的正式账簿、同步、Go 服务、备份迁移或官方发布；当前系统保护和独立解锁仅服务隔离合成实验。实验 schema / ABI 和工具链候选尚未成为生产契约。

实际调用、所有权、取消、模块边界、构建入口与工具链候选见[移动核心实验](../development/mobile-core-prototype.md)，正式门槛见[两端矩阵](../development/mobile-core-validation.md)，详细证据与问题收口见[本月 devlog](../devlogs/2026-10.md)。

## 已完成

- 产品、领域、隐私、自部署、数据可迁移性与家族 UI 规格已建立；M0-A 的首笔 / 关键状态规格和[共同账务材料](../validation/accounting-cases.md)已完成。
- 仓库治理已完成首次远端引导、真实自动 PR 检查、master Ruleset、正常合并及 dev 快进回流。治理事实与历史证据保留在 devlog；日常串行工作继续在 dev。
- 盘点 Flutter / Dart、Rust、Android SDK / NDK、Xcode、模拟器与可用设备；初始未发现连接真机。缺少的 JDK 与 bridge 生成工具安装在项目忽略目录，补齐固定 Rust 的两个 iOS targets，未修改全局默认版本或用户签名身份。
- Rust 核心真实保存修订 / tombstone / 候选，以单事务写事实、outbox 和序号；共同材料覆盖转账、退款、固定差额、修订 / 删除 / 恢复、归档和候选选择。回滚测试在 outbox 写入点实际拒绝，不使用 mock 成功。
- 两种 bridge 共用核心和材料，Dart isolate 执行；C ABI 有长度 / 句柄 / 释放限制，FRB binding 由固定工具生成。取消 UI 等待不会宣称底层事务回滚。
- 已接入真实 Rust / Flutter 检查及 PR `Core Quality`，`Candidate Quality` 严格汇总两项组件；未知语言工程仍阻断。新增组件仅在本地验证，尚未取得本批远端 CI 证据。
- 登记[第三方来源](../../THIRD_PARTY_NOTICES.md)，native、SQLCipher / OpenSSL 及 Rust 标准库 notices 随实验包保留；23 个工程 PNG 匹配固定 Flutter SDK / 官方模板图像包。

## M0-B 已有验证

下表记录 M0-B 批次收口时的结果与包版本；后续 M0-C 按批次分别列出，不把历史计数当作当前总数。

| 目标 | 实际结果 | 证据边界 |
| --- | --- | --- |
| macOS arm64 宿主 | Rust 8 项核心 / 进程 / C ABI 测试、fmt / clippy / C header、Flutter 分析和 7 项测试通过 | 两种 bridge 实际加载 SQLCipher；UI 失败与停止等待另有 widget 测试，不外推移动设备 |
| iOS 26.5 arm64 Simulator | 两种 bridge 各完成 10 案例 / 34 检查点，最终报告与合成界面截图已保存 | debug Flutter runner + Rust release 核心；模拟器不证明真机 release 或 Keychain 保护 |
| iOS arm64 真机目标 | 未签名 release App 构建、native arm64 / 依赖与 notices 检查通过，本地归档已保存 | 未签名、未安装到真机，不代表 App Store 或真机验收 |
| Android arm64 核心 | release Rust / SQLCipher / OpenSSL 已编译，动态依赖仅 Android 系统库；ELF LOAD 对齐为 16 KiB | 这是 native 构建和静态资产证据，不证明最低 API 或 16 KiB 系统运行 |
| Android arm64 真机目标 | release APK 构建通过（23,620,390 bytes）；隔离测试签名、arm64、16 KiB ZIP / Rust ELF 对齐与 notices 检查通过 | 不是正式发布签名；未安装真机，debug 模拟器结果不替代 release 运行 |
| Android API 35 arm64 模拟器 | 复用 Pixel_9_Pro，系统页大小 16384；两种 bridge 各完成 10 案例 / 34 检查点，完整结果与 iOS 一致 | debug Flutter runner + Rust release 核心；合成界面已复核，不外推最低 API 或真机 |
| 两端模拟器有界压力 / 故障 | 每端每种 bridge 6200 次错误返回调用，句柄归零，panic / ABI / native 错误明确拒绝；4 写 1 读共享库及实际 UI 取消通过 | 短时 RSS 门槛通过，不是长期泄漏、帧耗时或真机资源结论 |
| 两端跨进程恢复 | 两种 bridge × 三个事务中断点，每端 6 组 SIGKILL / 新 PID / 重放 / 合成目录清理通过；完整审计结果跨端一致 | 公开固定合成材料重开库，不代表生产解锁、系统密钥或断电恢复 |
| 仓库治理 | 24 条检查器回归及 162 个仓库文件检查通过 | 与业务测试分别统计；最后提交前再次复核实际清单 |

两种 bridge 的核心验证包含真实加密库、事务拒绝后的完整回滚、重复意图不重复入队、同进程关闭重开和随机错钥拒绝。金额 / 日期 / 无效意图按原共同材料校验。新增实验已取得真实进程强杀与共享账库并发证据；尚未执行断电 / 文件系统丢写、长期内存压力或真实设备锁屏 / 生物识别失效实验；iOS Simulator 精确删除本批保护项的结果见下表。公开恢复 fixture 不证明密钥保密性；仍未取得完整 WAL / 临时文件 / 内存边界证据。


### M0-C 2026-10-14 切片

| 目标 | 结果与范围 |
| --- | --- |
| 宿主密码与持久化 | RFC 5869 / RFC 9106 / XChaCha 草案独立向量、错误材料 / 篡改 / 超限拒绝、文件归属 / 排他 / 发布拒绝、六点真实子进程强杀与恢复通过；当批全部 Rust 测试 14 项 |
| 两端模拟器核心 | 每端两 bridge × 7 组：正常强杀新 PID、独立口令与恢复材料、错误口令文件未变、六处 bootstrap / local-wrap 中断；每端 14 组报告已保存并精确清理 |
| iOS 系统保护 | 两 bridge 的 WhenPasscodeSetThisDeviceOnly + userPresence API 创建 / 读取、重启、精确项删除、独立恢复与重建后再重启通过；Simulator 不证明真实认证强度 |
| Android 系统保护 | API 35 现有设备 `deviceSecure=false`、平台代码 11；项缺失和创建 unsupported 明确返回，未创建低保护替代项，日常解锁 / 重建未验收 |
| 资源样本 | iOS 创建 274–303 ms、口令打开 181–184 ms、观察峰值约 401 MiB；Android 创建 220–319 ms、口令打开 289–461 ms、峰值约 376 MiB；各为少量 debug runner 样本，不冻结真机 profile |
| 宿主 Flutter | 11 项测试及静态分析通过；包含 UI 取消 / 晚结果、模拟后台清理和平台只读字节缓冲区回归 |
| 两端实际 UI | 每端两 bridge 的错误口令、锁定取消、晚结果抑制、再次输入及原库重试通过；合成结果截图已复核 |
| M0-C 目标包 | iOS arm64 未签名 release App、Android arm64 测试签名 release APK（23,974,098 bytes）构建通过；包内 notices 匹配，Android v2 签名 / ZIP 与 Rust ELF 16 KiB 对齐通过；未在真机运行 |

该批保真材料为 A01，不能外推到 A11。真实后台任务预览、系统认证取消 / 拒绝 / 暂不可取、硬件等级、设备重启和真机 KDF 资源仍是后续门槛。SIGKILL 不替代断电，独立本机恢复不替代空白设备归档恢复。

### M0-C 2026-10-19 宿主补充

| 目标 | 实际结果与证据边界 |
| --- | --- |
| Android 条件只读核对 | adb / emulator 未运行；现有 Pixel_9_Pro 为 API 35、Pixel_10_Pro 为 API 37.1，均 arm64。未启动设备或修改设置，运行时 `deviceSecure` / 认证能力未核实，不把历史 unsupported 当作今日检测结果 |
| A11 非空恢复 | 同库保留 2 个候选、1 个当前删除、8 个修订和 7 个 outbox 操作；宿主实际 WAL 强杀、新 PID 口令 / 恢复材料读取、错误材料与损坏封装保全、原意图重放及原六点强杀通过 |
| 文件发布与文件边界 | 四类文件 × 六种模式通过：正数短写续写、零写 / IO 拒绝、文件同步 / 发布拒绝、目录同步结果未定。宿主实际 DB / WAL / SHM / 回滚日志、checkpoint、回滚与明文正向对照通过；这是显式注入和宿主文件证据，不是实际磁盘故障、完整临时文件审计或断电结论 |
| 宿主检查 | Rust 18 项测试、fmt / clippy / C header；Flutter 13 项测试、格式和严格静态分析通过。两个 bridge 实际调用 A11 与发布故障，Dart isolate 不作为跨 PID 证据 |
| 两端目标编译 | iOS arm64（iOS 15 候选）、Android arm64（API 24 候选）native release 核心编译通过；未重新打完整 App 包、未安装或运行设备，不替代既有包或真机验收 |
| 仓库治理 | 24 条检查器回归及 182 个文件检查通过，包含新增测试、文档路由和根入口一致；没有本轮远端 CI 证据 |

复现与语义见[文件发布故障](../development/mobile-core-prototype.md#78-非空账务与文件发布故障)，该宿主批次日志、只读核对摘要及 native 产物保存在忽略目录 `local-evidence/m0-c/2026-10-19/`。移动端后续复验见下表；DB pager 短写 / fsync、完整临时文件与断电边界仍未关闭。

### M0-C 2026-10-19 移动复验

| 目标 | 实际结果与证据边界 |
| --- | --- |
| 现有设备与 Android 认证 | 复用 Pixel_9_Pro（API 35 arm64、16384-byte 页）及 iPhone 17 Pro（iOS 26.5 arm64 Simulator）。Android 首末能力一致：`deviceSecure=false`、`supported=false`、平台代码 11；双 bridge 均明确拒绝创建，系统日常解锁 / 失效未执行，锁屏设置保留 |
| 两端双 bridge 核心 | 每端 16 组：正常强杀 / 新 PID、原六处中断、A11 非空 WAL 强杀与原意图重放；四次 WAL 观测均为 296672 bytes。2 个候选、1 个删除、8 个修订、7 个 outbox 及完整摘要保全，错误口令 / 恢复材料拒绝后文件不变 |
| 两端文件故障 | 每端两 bridge × 四类文件 × 六种模式，共 48 组；故障后重启，核对发布拒绝 / 结果未定、独立恢复及活动保护重试。属于显式封装文件 IO 注入，不是 DB pager 故障、实际磁盘损坏或断电证据 |
| iOS 系统保护 | 双 bridge 的精确 Keychain 项创建 / 读取、重启解锁、删除、A11 独立恢复及重建后再重启通过；限 Simulator API，硬件等级与真实认证强度未测 |
| 两端实际 UI | 双 bridge 的错误口令、锁定取消、晚结果抑制、原库重试及 A11 完整审计一致通过；4 张输入已清空的合成结果截图已复核 |
| 宿主复核 | Flutter 格式、严格静态分析和 13 项测试通过；控制器独立预期能拒绝丢失候选 / 删除 / outbox 的报告。本批 Rust 生产实现未改，18 项 Rust 结果仍归前述宿主批次 |
| 仓库治理 | 24 条检查器回归及 182 个文件检查通过，diff 文本卫生通过；没有本批远端 CI 证据 |

本批使用 debug Flutter runner + release Rust 核心，没有真机或完整 release App 运行证据。报告、截图、日志和资源清理记录单独保留在忽略目录 `local-evidence/m0-c/2026-10-19-mobile/`，不覆盖旧批次。

## 下一顺位与待冻结

B03–B07 的有界模拟器切片及 M0-C 独立算法向量已有证据。下一轮按下列顺序补齐缺口；设备条件未具备时先做不依赖设备认证的文件与保真验证，不将未执行项改记通过。实验与构建复现约束统一见[实验专题](../development/mobile-core-prototype.md)。

### 接下来的事项

1. **补 Android 系统认证闭环（C06–C08 / C12）。** 先只读核对现有设备是否具备安全锁屏与所需认证能力，复用已有设备；若必须改变共享模拟器锁屏设置或写入真机，另明确设备、影响和恢复方式后确认范围。取得两 bridge 的实际创建 / 日常解锁、取消 / 拒绝 / 暂不可取、精确项缺失后的独立恢复、重建与跨 PID 再解锁证据；真实安全设置变化导致的失效单独验证。
2. **补 DB pager 与完整文件边界（B07 / B09、C02 / C05 / C08）。** A11 跨进程恢复和封装文件发布故障已有宿主及两端双 bridge 模拟器证据；继续补 DB pager 短写 / fsync、移动目标 DB / journal 与实际临时文件及缓存边界。宿主扫描不替代移动扫描，强杀和显式注入不替代断电持久化结论。
3. **安排两端真机 release 与生命周期验收（B01–B06、C01 / C04 / C06–C10 / C12）。** 核对可用机型、系统、签名与安装范围后执行；采集正常 / 错误 / 取消、真实系统失效、设备重启、后台与任务预览，以及 KDF 耗时 / 峰值内存和较长压力样本。按实际能力记录硬件保护，不能把目标包构建或模拟器结果当作真机通过。
4. **收齐证据后再做选型与阶段评审。** 补锁定依赖的安全公告 / 供应链审查，汇总两 bridge 的维护成本与真机结果，再决定生产 ABI、系统矩阵、SQLCipher / 系统保护及 KDF profile，必要时形成 ADR。M0-D / E 继续按路线图验证同步与归档；独立本机恢复不等于空白设备归档恢复。

### 待冻结项

| 待冻结项 | 所需证据 |
| --- | --- |
| bridge / 生产 ABI / 具体依赖 | 两端真机 release、编码 / 所有权 / 压力 / 并发 / 取消与维护取舍 |
| 最低系统 / ABI / 机型 | 候选最低系统的实际运行；本批 API 24 / iOS 15 仅为 App 构建候选 |
| SQLCipher 与密钥保护配置 | WAL / 临时文件、升级、bootstrap 与系统密钥失效后的独立本机恢复 |
| KDF profile | 已有独立向量与实验参数拒绝证据，仍需真机时间 / 峰值内存、错误 / 取消行为与资源预算 |
| HTTP / envelope / 业务 schema / 归档 | M0-D / E 的条件提交、删除、迁移冻结、认证 / 保真与恢复验证 |

没有因模拟器通过直接宣布 M0 退出、进入 M1 或公开 MVP 完成。录入效率、大字号、键盘、安全区和生产 UI 仍待实际用户流程验证。

## 工作区与外部状态

`origin` 为 [laugh0608/RadishLedger](https://github.com/laugh0608/RadishLedger)，默认分支 master，日常分支 dev；[master Ruleset](https://github.com/laugh0608/RadishLedger/rules/24995092) 的 required context 保持 Candidate Quality，dev 不强制 PR / checks。历史治理 PR 与验证见 devlog。

10 月 14 日四项提交为 b29ac27（双桥接实验基线）、c6e1a36（Android 构建与模拟器收口）、0c71bc0（压力 / 异常 / 强杀恢复）和 c88a2eb（M0-C bootstrap 与独立恢复）；此前 M0-A 本地提交为 ae4cc7b。日终另做文档核对与本地提交。本批没有 push、PR、远端设置变更、tag、Release、上传、发布或部署。初始 version.json 与实验包版本不代表稳定发行。AGENTS / CLAUDE 保持逐字一致。

M0-B 批次复用 Pixel_9_Pro 与 iPhone 17 Pro（iOS 26.5），恢复实验的合成 fixture / 控制文件已清理，两端 App 均已恢复普通入口；设备定义、其他应用与快照保留，未创建新设备。本批启动的模拟器与 adb 已停止，构建 / 调试进程的退出状态已复核。按项目所有者要求，已删除本批 RadishLedger-M0B iOS 设备、RadishLedger_M0B Android AVD，以及确认无其他设备使用的 Android API 35 google_apis_playstore arm64-v8a 镜像。11 份既有验证证据逐文件 SHA-256 复核一致，构建产物和其他工具缓存保留。当时保留 iOS 设备 11 个、Android AVD 2 个（Pixel_9_Pro、Pixel_10_Pro），另有 watchOS 设备 5 个。后续直接复用现有模拟器，执行前核对设备 ID、系统版本和运行状态，不将旧设备结果当作新目标的验证结果。

10 月 14 日 M0-C 批次结束后，core harness 的全部登记账库 / 保护引用已精确清理，UI 验证在 finally 清理本次 fixture，Flutter drive 随后卸载测试 runner。两端均重新安装并恢复普通入口，核对本项目 M0-C 目录不存在或为空后停止 App；本次启动的 iPhone / Pixel 模拟器和无其他设备连接的 adb 已关闭，设备定义与锁屏设置保留。18 份本地证据 / 产物已记录 SHA-256；最终宿主 Flutter 11 项测试、Rust 14 项测试及严格静态检查通过，仓库治理 24 条回归 / 180 个文件通过。本批只做本地提交，不 push。

2026-10-19 首个宿主批次只读核对 Android 配置与进程，未启动设备，已本地提交为 `9807d28`。后续按授权复用上述两台现有模拟器完成移动复验：控制器登记账库与系统引用全部清理，UI fixture 清理后卸载测试 runner，两端重装普通入口并核对 M0-C 目录不存在或为空后停止 App；本次启动的模拟器和无其他设备连接的 adb 已关闭。设备定义、锁屏、其他应用及快照保留。首次 simctl 盘点触发 Xcode 自动组件初始化并替换旧 CoreSimulator 服务，已记录实际环境变化，未主动安装 / 升级工具。依赖声明和 lockfile 未变；仅本地提交，不 push / PR / 发布。

## 当前停止线

- 只使用固定合成材料和本批隔离目录；模拟器复用既有设备，移动实验数据限定在本项目 App 沙盒，宿主实验仅使用本次独占临时目录，不导入真实账单，不读取私人 UI 参考或处理个人金融内容。
- 系统项仅限本批随机身份的 Android alias `radishledger.m0c.` / iOS service `com.radishledger.m0c`，创建与精确删除已获授权；不操作宿主用户密钥链、正式签名 / 生产密钥，不改变共享设备锁屏设置。
- 不将临时内存密钥当作可进入 M1 的解锁 / 恢复基线，不公开分发实验签名包。
- 不铺大量生产页面、Go 服务、同步端点或可部署模板，不跳过 M0 风险验证。
- 不推送本批工程或扩大既有远端引导授权；不改写共享历史或更新兄弟项目。
