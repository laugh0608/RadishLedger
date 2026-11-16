# ADR-0005 Android 受限内部开发的桥接候选

已接受，适用范围为 Android 合成数据内部流程。记录日期为 2026-11-16（Asia/Shanghai）。

## 背景

内部开发按既有决定，在证据核对后只使用一个桥接候选。E15 宿主新建归档切片已通过。两种候选共用 Rust 账务与 SQLCipher 核心、版本化非秘密 JSON 以及独立的有界秘密字节通道。[ADR 0004](0004-internal-development-and-platform-exits.md) · [宿主归档验收](../validation/portability-cases.md#511-e15-宿主新建归档文件) · [移动核心契约](../development/mobile-core-prototype.md#1-一个核心两个调用候选)

## 决定与代价

首个 Android 内部本地流程采用现有薄 C ABI / Dart FFI。新增调用沿用 Dart 工作 isolate、明确版本和有界缓冲区。业务规则、持久状态和错误原因由 Rust 提供。新入口验收时共同核对编码、缓冲区归还、取消与晚结果。FRB 保留为实验对照。

| 候选 | 现有接入与维护成本 |
| --- | --- |
| 薄 C ABI / Dart FFI | 已有 C 头文件、Dart 适配、结果句柄释放与独立秘密缓冲区用例。小批增量可直接复用。改动需人工同步声明和调用端，并复核 unsafe 指针、所有权、长度、错误与重复释放 |
| flutter_rust_bridge | 生成绑定能减少复杂跨语言类型的手写映射。当前探针仍共用 JSON 与有界秘密输入，需要额外维护固定 runtime、生成器、cargo-expand 和两端生成绑定。新增入口需要生成与真实加载复验 |

两种候选在已有 Android 模拟器记录中均完成恢复、错误、取消和各 6200 次压力调用，薄 ABI 活跃结果缓冲区归零。既有 Android 产物和构建链可复现双候选实验，当前只有同一 App 的短时 RSS 采样和组合包容量，单桥接 release 包大小与真机资源验收仍待实测。本次按小范围接入和维护成本选择。[Android 运行记录](../development/mobile-core-prototype.md#正式工作区接入结果)

## 准入核对与正式冻结

| 内部条件 | 已有证据 |
| --- | --- |
| 共同账务预期 | 同一 Rust 核心复用 A01 / A09 共同材料，A11 的事实、候选、删除与金额另有独立预期 |
| 加密、错误保全和重启 | 现有 SQLCipher 核心、Android 双桥接恢复 / 文件故障、宿主跨进程事务恢复及两个密码提供方的既有证据 |
| 独立恢复 | E15 磁盘文件回读后独立 SQLCipher 暂存，`0.2` 宿主跨代保存与恢复保持原事实、历史及金额 |
| 单一候选及平台缺口 | Android 内部采用薄 C ABI / Dart FFI，只使用合成数据。认证或平台保护不可用时，对应操作保持阻断 |

Android M1 与 iOS M2 所需的真机 release、系统认证、密钥恢复、文件保护、生命周期及资源项继续按原矩阵取证。生产桥接、ABI、依赖、最低系统、归档和 KDF 依各自验收冻结。[验证矩阵](../development/mobile-core-validation.md) · [阶段与顺位](../status/current.md#接下来的事项)
