# RadishLedger（萝卜账簿）

本地优先的私人账簿，支持端到端加密同步、官方服务与自部署。

RadishLedger 面向个人日常记账，提供 Android 与 iOS 官方客户端。用户可以只在本机记账，也可以使用同一个安装包连接官方端点或自己的兼容后端；本地记账、自部署和数据迁出不依赖官方服务在线。

本仓库的正式文档定义产品目标、第一版公开 MVP、架构与工程规范。实际阶段、已完成内容和下一步统一见[当前状态](docs/status/current.md)，不能从规划推断功能已经实现。

## 产品承诺

- 日常记账离线可用，保存成功以本地事务提交为准。
- 流水是账务事实，余额和统计可重建；不静默覆盖冲突或丢弃未同步修改。
- 默认保护本地数据，同步内容端到端加密，报表和搜索在客户端执行。
- 官方与自部署使用相同核心协议，服务器在运行时选择，无需重新编译客户端。
- 完整加密归档可脱离原服务器恢复；服务地址切换和账簿迁移分别处理。
- 个人自部署获得明确、免费的许可，不需要逐个申请或向官方激活。

## 第一版范围

第一版公开 MVP 收敛为单人、单账簿、人民币 CNY，支持多个资金账户、手动收入 / 支出 / 转账 / 普通退款、分类、流水查询、月统计、CSV 交换、加密备份恢复、双端同步和两种服务端点。

多账簿、预算、年度专门报表、复杂关联退款、自动记账、附件、家庭共享、多币种、银行直连、AI、桌面和 Web 记账端后置。完整范围与验收编号见[产品规划](docs/product-scope.md)及[MVP 路线图](docs/mvp-roadmap.md)。

## 技术方向

| 部分 | 职责 |
| --- | --- |
| Flutter | 手机界面、输入、图表、平台交互和状态展示 |
| Rust 客户端核心 | 账务规则、加密本地库、统计、密钥、同步与冲突 |
| Go 单体服务 | 身份、会话、设备、访问控制、密文存储和条件提交 |
| PostgreSQL | 服务端持久化、事务、变更流与运维备份 |

Rust 核心与 SQLCipher 是优先验证的实现方向；移动端桥接、依赖版本、系统密钥能力和最低系统版本需经过 M0 验证再冻结。详见[架构](docs/architecture.md)及[隐私与加密方案](docs/privacy-threat-model.md)。

## 文档入口

- [完整文档导航](docs/README.md)
- [产品规划与范围](docs/product-scope.md)
- [MVP 里程碑与验收](docs/mvp-roadmap.md)
- [账务模型与统计口径](docs/domain-model.md)
- [技术架构](docs/architecture.md)
- [同步协议与连接边界](docs/sync-protocol.md)
- [隐私、加密与密钥生命周期](docs/privacy-threat-model.md)
- [自部署与官方托管](docs/self-hosting.md)
- [导入导出、恢复与迁移](docs/data-portability.md)
- [移动端体验与家族视觉](docs/design/mobile-experience.md)
- [开发规范](docs/development/README.md)
- [移动核心实验与复现](docs/development/mobile-core-prototype.md)
- [协作与仓库治理](docs/governance/README.md)
- [贡献指南](CONTRIBUTING.md)
- [安全报告](SECURITY.md)
- [许可策略](docs/licensing-strategy.md)

## 开发与许可

`dev` 是日常串行开发与集成分支；`master` 是通过 PR 晋级的稳定主线。分支、PR、合并、回流与 GitHub 策略见[仓库治理](docs/governance/repository-governance.md)，实现与版本规则见[开发规范](docs/development/README.md)。创建远端、推送、发布和部署按实际任务授权执行。

仓库采用 [RadishLedger Source-Available License 1.0](LICENSE)，属于源码可见授权，不是开放源码许可证。个人可免费获取、构建、使用和自部署，也可为自己的实例进行私有修改；商业服务和再分发按许可证处理。用户自己的账务数据不受软件许可证限制。

第三方依赖、Flutter 模板与实验包内许可见[第三方来源](THIRD_PARTY_NOTICES.md)，各自授权独立于项目许可证。
