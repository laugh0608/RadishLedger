# 当前状态

更新日期：2026-10-14（Asia/Shanghai）。

## 当前结论

项目处于 M0-B 移动核心接入实验，首批成果已本地提交，Android 构建断点已解决，两端模拟器核心调用和真机目标构建均已取得本批证据。项目所有者明确本批先完成模拟器验证和真机目标构建，真机运行留到后续；M0-B / M0-C 的正式退出条件保持不变。

已有 Flutter 合成验证 App、同一 Rust 领域 / SQLCipher 事务核心，以及薄 C ABI / Dart FFI 与 flutter_rust_bridge 两种候选。它们执行同一份人工预期材料，不接收真实账单。没有可供用户使用的账簿、系统密钥 / 独立解锁、同步、Go 服务、备份迁移或官方发布。实验 schema / ABI 和工具链候选尚未成为生产契约。

实际调用、所有权、取消、模块边界、构建入口与工具链候选见[移动核心实验](../development/mobile-core-prototype.md)，正式门槛见[两端矩阵](../development/mobile-core-validation.md)，详细证据与问题收口见[本月 devlog](../devlogs/2026-10.md)。

## 已完成

- 产品、领域、隐私、自部署、数据可迁移性与家族 UI 规格已建立；M0-A 的首笔 / 关键状态规格和[共同账务材料](../validation/accounting-cases.md)已完成。
- 仓库治理已完成首次远端引导、真实自动 PR 检查、master Ruleset、正常合并及 dev 快进回流。治理事实与历史证据保留在 devlog；日常串行工作继续在 dev。
- 盘点 Flutter / Dart、Rust、Android SDK / NDK、Xcode、模拟器与可用设备；初始未发现连接真机。缺少的 JDK 与 bridge 生成工具安装在项目忽略目录，补齐固定 Rust 的两个 iOS targets，未修改全局默认版本或用户签名身份。
- Rust 核心真实保存修订 / tombstone / 候选，以单事务写事实、outbox 和序号；共同材料覆盖转账、退款、固定差额、修订 / 删除 / 恢复、归档和候选选择。回滚测试在 outbox 写入点实际拒绝，不使用 mock 成功。
- 两种 bridge 共用核心和材料，Dart isolate 执行；C ABI 有长度 / 句柄 / 释放限制，FRB binding 由固定工具生成。取消 UI 等待不会宣称底层事务回滚。
- 已接入真实 Rust / Flutter 检查及 PR `Core Quality`，`Candidate Quality` 严格汇总两项组件；未知语言工程仍阻断。新增组件仅在本地验证，尚未取得本批远端 CI 证据。
- 登记[第三方来源](../../THIRD_PARTY_NOTICES.md)，native、SQLCipher / OpenSSL 及 Rust 标准库 notices 随实验包保留；23 个工程 PNG 匹配固定 Flutter SDK / 官方模板图像包。

## 本批验证

| 目标 | 实际结果 | 证据边界 |
| --- | --- | --- |
| macOS arm64 宿主 | Rust 4 项核心 / C ABI 测试、fmt / clippy / C header、Flutter 分析和 5 项测试通过 | 两种 bridge 实际加载 SQLCipher；UI 失败与停止等待另有 widget 测试，不外推移动设备 |
| iOS 26.5 arm64 Simulator | 两种 bridge 各完成 10 案例 / 34 检查点，最终报告与合成界面截图已保存 | debug Flutter runner + Rust release 核心；模拟器不证明真机 release 或 Keychain 保护 |
| iOS arm64 真机目标 | 未签名 release App 构建、native arm64 / 依赖与 notices 检查通过，本地归档已保存 | 未签名、未安装到真机，不代表 App Store 或真机验收 |
| Android arm64 核心 | release Rust / SQLCipher / OpenSSL 已编译，动态依赖仅 Android 系统库；ELF LOAD 对齐为 16 KiB | 这是 native 构建和静态资产证据，不证明最低 API 或 16 KiB 系统运行 |
| Android arm64 真机目标 | release APK 构建通过（23,473,046 bytes）；隔离测试签名、arm64、16 KiB ZIP / Rust ELF 对齐与 notices 检查通过 | 不是正式发布签名；未安装真机，debug 模拟器结果不替代 release 运行 |
| Android API 35 arm64 模拟器 | 复用 Pixel_9_Pro，系统页大小 16384；两种 bridge 各完成 10 案例 / 34 检查点，完整结果与 iOS 一致 | debug Flutter runner + Rust release 核心；合成界面已复核，不外推最低 API 或真机 |
| 仓库治理 | 24 条检查器回归及 153 个仓库文件检查通过 | 与业务测试分别统计；最后提交前再次复核实际清单 |

两种 bridge 的核心验证包含真实加密库、事务拒绝后的完整回滚、重复意图不重复入队、同进程关闭重开和随机错钥拒绝。金额 / 日期 / 无效意图按原共同材料校验。没有执行断电 / 进程崩溃恢复、共享账库并发、长期内存压力或系统密钥失效实验；只检查加密主文件 header 与错钥，未取得完整 WAL / 临时文件 / 内存边界证据。

## 下一顺位与待冻结

下一批继续 B03–B06 的压力、panic / ABI 故障、共享写入并发、进程终止及跨重启结果核对。本批 Android 的两处构建断点已收口：App 与 integration_test 统一使用 AGP 默认 Build-Tools 36.0.0，模式切换保留 Flutter 平台文件生成流程，避免 --no-pub 残留错误的插件注册。随后推进 M0-C 的 bootstrap、日常保护与独立解锁、系统密钥取消 / 失效、KDF 向量与资源证据；真机 release 调用仍作为后续必需验收。

| 待冻结项 | 所需证据 |
| --- | --- |
| bridge / 生产 ABI / 具体依赖 | 两端真机 release、编码 / 所有权 / 压力 / 并发 / 取消与维护取舍 |
| 最低系统 / ABI / 机型 | 候选最低系统的实际运行；本批 API 24 / iOS 15 仅为 App 构建候选 |
| SQLCipher 与密钥保护配置 | WAL / 临时文件、升级、bootstrap 与系统密钥失效后的独立本机恢复 |
| KDF profile | 独立向量、真机时间 / 峰值内存、上下界和错误 / 取消行为 |
| HTTP / envelope / 业务 schema / 归档 | M0-D / E 的条件提交、删除、迁移冻结、认证 / 保真与恢复验证 |

没有因模拟器通过直接宣布 M0 退出、进入 M1 或公开 MVP 完成。录入效率、大字号、键盘、安全区和生产 UI 仍待实际用户流程验证。

## 工作区与外部状态

`origin` 为 [laugh0608/RadishLedger](https://github.com/laugh0608/RadishLedger)，默认分支 master，日常分支 dev；[master Ruleset](https://github.com/laugh0608/RadishLedger/rules/24995092) 的 required context 保持 Candidate Quality，dev 不强制 PR / checks。历史治理 PR 与验证见 devlog。

首批实验成果已保存为本地提交 b29ac27；此前 M0-A 本地提交为 ae4cc7b。Android APK 构建和模拟器验证已通过，修复与结论作为后续本地提交保存。本批没有 push、PR、远端设置变更、tag、Release、上传、发布或部署。初始 version.json 与实验包版本不代表稳定发行。AGENTS / CLAUDE 保持逐字一致。

本批启动的 Pixel_9_Pro、adb 与构建 / 调试进程均已停止，设备定义、其他应用与快照保留，未创建新设备。按项目所有者要求，已删除本批 RadishLedger-M0B iOS 设备、RadishLedger_M0B Android AVD，以及确认无其他设备使用的 Android API 35 google_apis_playstore arm64-v8a 镜像。11 份既有验证证据逐文件 SHA-256 复核一致，构建产物和其他工具缓存保留。现有 iOS 设备 11 个、Android AVD 2 个（Pixel_9_Pro、Pixel_10_Pro），另有 watchOS 设备 5 个。后续直接复用现有模拟器，执行前核对设备 ID、系统版本和运行状态，不将旧设备结果当作新目标的验证结果。

## 当前停止线

- 只使用固定合成材料和本批隔离目录；模拟器复用既有设备，实验数据限定在本项目 App 沙盒，不导入真实账单，不读取私人 UI 参考或处理个人金融内容。
- 不操作用户密钥链、正式签名 / 生产密钥或用户设备；系统密钥实验先明确隔离对象与恢复边界。
- 不将临时内存密钥当作可进入 M1 的解锁 / 恢复基线，不公开分发实验签名包。
- 不铺大量生产页面、Go 服务、同步端点或可部署模板，不跳过 M0 风险验证。
- 不推送本批工程或扩大既有远端引导授权；不改写共享历史或更新兄弟项目。
