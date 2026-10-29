# RadishLedger 正式文档

本目录是产品、领域、架构与开发规范的正式来源。长期专题描述目标与边界，[当前状态](status/current.md)记录实际进度，历史过程进入 devlog。规划存在不表示产品能力已交付。

## 按问题进入

| 问题 | 文档 |
| --- | --- |
| 产品为谁服务、完整目标是什么 | [产品规划与范围](product-scope.md) |
| 第一版必须完成什么、如何分阶段 | [MVP 路线图](mvp-roadmap.md) |
| 金额、转账、退款和余额如何计算 | [账务模型](domain-model.md) |
| 各端使用哪些合成输入和数值预期 | [共同账务样例](validation/accounting-cases.md)、[单一 JSON 材料](validation/accounting-cases.json) |
| Flutter、Rust、Go 如何分工 | [技术架构](architecture.md) |
| 两端最小接入、加密与系统密钥如何验证 | [移动端核心验证矩阵](development/mobile-core-validation.md) |
| 合成移动核心实验如何调用、构建和复现 | [移动核心实验](development/mobile-core-prototype.md)、[第三方来源](../THIRD_PARTY_NOTICES.md) |
| 人工恢复、精确清理、诊断和锁屏实验如何执行 | [人工实验管理](development/mobile-core-prototype.md#713-人工实验页的精确失效清理与候选切换)、[非秘密报告](development/mobile-core-prototype.md#714-人工实验的有界非秘密报告)、[iOS 文件观测](development/mobile-core-prototype.md#715-ios-有界锁屏访问观测)、[设备操作单](development/mobile-core-validation.md#64-单批操作单与验收停止线) |
| 多端如何连接、提交、解决冲突 | [同步协议与连接边界](sync-protocol.md) |
| M0-D 如何复核事务、分页、域隔离与恢复 | [同步事务共同案例](validation/sync-cases.md)、[待冻结契约](sync-protocol.md#91-规格实现与冻结门槛) |
| 保护什么、谁持有密钥、如何恢复 | [威胁模型与加密](privacy-threat-model.md) |
| 如何交付自部署和官方服务 | [自部署与托管](self-hosting.md) |
| 如何导入、备份、恢复和换服务器 | [数据可迁移性](data-portability.md) |
| M0-E 如何复核批次、归档保真与安全切换 | [数据可迁移共同案例](validation/portability-cases.md)、[待冻结契约](data-portability.md#9-冻结条件与实现顺序) |
| 手机布局、家族视觉和私有参考如何约束 | [移动端 UI 规范](design/mobile-experience.md)、[UI 采用决策](adr/0003-ui-family-and-private-references.md) |
| 日常开发、分支、版本和验证怎么做 | [开发规范](development/README.md) |
| 协作、Git、PR、Rulesets 与自动化如何约束 | [治理导航](governance/README.md) |
| 漏洞、凭据或数据安全问题如何报告 | [安全报告](../SECURITY.md) |
| 源码可见与个人自部署的许可边界 | [许可策略](licensing-strategy.md) |
| 已接受的决定和后续决策入口 | [ADR 索引](adr/README.md) |
| 现在到哪一步、下一步做什么 | [当前状态](status/current.md)、[接下来的事项](status/current.md#接下来的事项) |
| 历史推进与验收记录 | [2026 年 10 月开发记录](devlogs/2026-10.md) |

## 来源与维护

- 产品能力及范围编号 R01–R12 只在产品范围定义；路线图引用编号，不维护第二份功能表。
- 账务公式与事件语义以领域模型为准；传输顺序不改变业务日期或金额。
- 共同账务案例的具体输入与人工预期只维护在验证 JSON；样例说明、页面与技术矩阵引用它，不复制另一套数值。样例结构版本不等于生产契约版本。
- 同步与公开 envelope 以同步方案为准；密钥与加密认证边界以威胁模型为准，二者同时变更时相互复核。
- 同步案例只维护合成调度与独立事务预期，金额仍引用共同账务 JSON；规格推演、真实 PostgreSQL、客户端状态机和端到端验证分别取证，不以文档或符号正文代替实现通过。
- 可迁移案例维护 CSV / 归档 / 暂存 / 迁移的输入范围与故障预期，账务继续引用同一 JSON；可逆文本编码、密码认证、保真集合和平台发布各自验证，不能以 CSV 有损投影或数据库实验替代完整恢复。
- 部署目标和客户端支持范围需用真实目标证据冻结，不能以框架支持矩阵替代产品验收。
- 根入口只保留定位、导航和启动约束；不要复制完整设计或当前批次。
- Git / PR / CI 策略以治理专题为准；配置与检查器保持同一口径。根 LICENSE 与第三方许可决定授权，不由贡献表单或自动化扩展。
- 技术规格冻结时创建对应 ADR 和版本化契约；当前草案不是已经稳定的 HTTP API、FFI ABI 或备份格式。
- 文档变更先更新最接近问题的正式来源，再同步必要的引用、ADR 和当前状态。

2026-10-12 的规划承接此前产品讨论，并根据 Radish 系列习惯重新收敛。此前聊天规划是历史讨论材料，进入开发后以本仓库对应专题为准。
