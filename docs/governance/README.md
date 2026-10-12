# 协作与仓库治理

本目录维护 RadishLedger 的长期协作、Git、PR 和自动化约束。规则适用于人工与 Agent；账务、加密和同步语义仍由对应产品专题定义。

| 问题 | 正式来源 |
| --- | --- |
| 如何读取任务、保护工作区、使用授权和交接 | [协作与执行](agent-collaboration.md) |
| 分支、提交、PR、合并、回流与仓库设置 | [仓库治理](repository-governance.md) |
| 为什么采用这套策略、参考了哪些项目 | [ADR 0002](../adr/0002-repository-governance.md) |
| 本地检查的依赖、命令和实际覆盖 | [检查脚本](../../scripts/README.md) |
| GitHub Ruleset 如何启用和复核 | [Rulesets 运维](../../.github/rulesets/README.md) |
| 编码、版本、跨语言实现与验证原则 | [开发规范](../development/README.md) |
| 实际阶段、检查证据与待办 | [当前状态](../status/current.md) |

## 规则归位

- `LICENSE` 决定软件授权；`SECURITY.md` 决定安全报告方式。
- 产品与技术专题定义长期行为；ADR 记录选择、理由和后果。
- 本目录定义稳定执行规则，根 `AGENTS.md` / `CLAUDE.md` 只保留启动即需生效的约束与路由。
- `.github/`、编辑器配置与检查器实施可验证规则；远端 GitHub 才能强制分支保护。
- 当前状态记录实际完成、下一步和临时门槛；月度 devlog 保留历史证据。

文件或自动化与正式规则不一致时，先查明哪一方过期，再在同一变更中收口。新规则优先进入最近的来源，不重复扩写入口。兄弟项目只提供经验，不成为本项目的配置来源、运行依赖或自动同步目标。
