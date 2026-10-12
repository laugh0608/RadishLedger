# 参与 RadishLedger

先阅读[当前状态](docs/status/current.md)、[文档导航](docs/README.md)及与任务相关的最少专题，并遵守[行为规范](CODE_OF_CONDUCT.md)。实现必须完整覆盖用户主路径和相应失败状态，不能把规划、演示或静态检查写成真实交付。详细工作区与授权规则见[协作与执行](docs/governance/agent-collaboration.md)。

## 工作流

1. 普通串行维护直接在 `dev` 推进；外部贡献从自己的主题分支向 `dev` 提 PR，并行写入和高风险隔离按需建立分支。
2. 先固定会影响产品、账务、协议、加密和数据格式的边界，再实施；现有授权范围内的常规改动无需重复确认。
3. 精确暂存任务文件，使用 Conventional Commits 和自己的 Git 身份，不添加 AI 署名。
4. 稳定阶段经 PR 晋级 `master`，优先 merge commit，合入后先回流 `dev`。允许 rebase merge、禁用 squash；共享分支不强推、不重写历史。完整规则见[仓库治理](docs/governance/repository-governance.md)。
5. 推送、创建 PR、tag / Release、发布、部署和外发信息需得到相应授权。

示例提交信息：

```text
docs(product): 明确第一版公开 MVP 范围
fix(sync): 保留超时提交的原始操作身份
test(ledger): 验证转账恢复后的双边余额
```

## 验证与交付

- 文档与治理使用 `./scripts/check-repo.sh`；定向入口和精确覆盖见[检查说明](scripts/README.md)。编号、领域样例与术语变化仍需专题复核。
- 后续实现按[开发规范](docs/development/README.md)执行相关 Rust、Go、Flutter 检查；真实双端、部署和恢复使用独立证据。
- PR 说明先讲具体问题与最终行为，再说明数据 / 兼容影响、实际验证、未验证范围及恢复方式；使用仓库模板，并在稳定主线 PR 说明回流安排。`Candidate Quality` 的实际业务覆盖随工程接入演进，不能推定真机、安全或恢复已经通过。
- 不提交真实账单、银行流水、令牌、密钥、恢复秘密、私人截图或未经授权的第三方资产。

## 许可与贡献

查看、个人私有构建 / 修改 / 自部署的权限见[根许可证](LICENSE)。公开反馈、原创建议和贡献按许可证条款处理；准备再分发源代码或修改版本、提供托管服务等单独授权行为时，按许可证取得相应许可。

贡献者必须有权提交其内容。除另有书面约定，已接收贡献按根许可证的贡献条款处理。第三方代码、字体、图标和依赖保留独立许可与 notices。

缺陷和提案使用 Issue 表单，只提交合成复现。安全问题按 [SECURITY.md](SECURITY.md) 私下报告，不在公开讨论中附上可用攻击材料、真实数据或凭据。
