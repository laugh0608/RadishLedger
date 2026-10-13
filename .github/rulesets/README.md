# GitHub Rulesets 的启用与复核

本目录声明 RadishLedger 的目标保护策略，完整规则见[仓库治理](../../docs/governance/repository-governance.md)。模板提交不等于远端已经生效，实际状态见[当前状态](../../docs/status/current.md)。

`master-protection.json` 精确保护 `master`：PR、已解决会话、严格的 `Candidate Quality`、禁止删除与强推，允许 merge / rebase。单维护者审批为 `0`；管理员角色只在 PR 内可 bypass，必须有例外授权和记录。`dev` 不设强制 Ruleset。模板不加入 commit-message 正则、线性历史、签名或 tag 规则。

## 首次引导

1. 明确目标 `owner/repo`、可见性、GitHub 套餐与创建 / 推送 / 设置的授权。读取已有分支、Rulesets、旧 branch protection、默认分支与合并设置，不覆盖其他有效约束。
2. 完成精确初始提交与本地检查。没有远端稳定分支时，在本地确认的基线 commit 上首次建立 `master` 与 `dev` 并推送；这是规则启用前的一次引导动作，不能据此保留日后直推例外。
3. 设置远端默认分支为 `master`。应用 [repository-settings.json](../repository-settings.json) 中的已审阅字段，特别关闭 squash 和自动删 head branch，避免删除长期 `dev`。
4. 先确认仓库实际允许运行 GitHub Actions，必要时在仓库 Actions 页面及 Settings -> Actions -> General 完成启用，再确认默认分支中的 workflow 已启用。不能只从 permissions API 的 `enabled: true`、workflow 文件存在或一次手动成功推定自动 PR 触发可用；启用后用新的 PR 事件取得实际证据。手动运行 `PR Checks` 并用治理 PR 验证 `pull_request -> master` 实际产生并通过 `Repo Hygiene` 和 `Candidate Quality`。首次登记受阻时，先通过只包含已检查治理材料的初始化 PR 将可执行 workflow 纳入默认分支；此时保护尚未启用，这次引导合并不作为门禁证据。规则启用后另行验证受保护 PR，不能把初始化路径保留为日后例外。
5. 读取规则实际来源和作用范围；无同范围 Ruleset 才创建，有则读取 ID 后精确更新。模板中 Admin 的 RepositoryRole ID `5` 必须按目标 API 支持核对。
6. 核对有效规则、合并选项、严格检查、会话解决、bypass 与分支匹配。direct push / force push / deletion 的阻断先用规则读回和 GitHub UI 判断；确需探测写入时必须单独授权，不能拿真实稳定历史冒险。
7. 测试 PR 合并后回流 `dev`，将 PR / workflow / 规则读回证据记入 current 和 devlog。只有真实取得的行为才记录为通过。

Ruleset 的创建检查例外关闭；因此先建立两条分支并产生真实检查，再启用保护，避免空仓库引导死锁。已有远端时不重新执行初始化或覆盖分支。

若真实手动基线检查已成功并核对来源，而自动 PR 触发仍未验证，可先启用严格 Ruleset 来阻断缺少检查的晋级；验证 PR 保持未合并，按 current / devlog 明确记录待验证项。规则应用、自动 PR 触发与受保护的正常合并是不同证据，不通过 bypass 或手写成功状态补齐缺失检查。

## 精确应用示例

以下命令只是操作说明，执行前必须解析目标并获得相应授权；不会由检查脚本自动执行。

```bash
# 先读取并保存实际状态，核对仓库与现有 rule ID。
gh api repos/OWNER/REPO
gh api repos/OWNER/REPO/rulesets

# 默认分支已存在后，只更新模板列出的仓库字段。
gh api repos/OWNER/REPO --method PATCH --input .github/repository-settings.json

# 没有同范围规则时才 POST。
gh api repos/OWNER/REPO/rulesets --method POST --input .github/rulesets/master-protection.json

# 已存在时，用查明的 ID 更新，不能再 POST 一个重复规则。
gh api repos/OWNER/REPO/rulesets/RULESET_ID --method PUT --input .github/rulesets/master-protection.json

# 更新后复核详情与作用到 master 的有效规则。
gh api repos/OWNER/REPO/rulesets/RULESET_ID
gh api repos/OWNER/REPO/rules/branches/master
```

更新前保存原始配置、记录拟改字段；更新后比较有效状态，回滚只恢复已授权字段。仓库合并选项和 Ruleset 两者共同作用，不能只检查其中一个。

required context 不采用其他项目的 App / installation ID。本模板的 `integration_id` 来自本仓库真实检查记录核对的 GitHub Actions App，实际来源证据见 current / devlog；应用时复核来源，精确更新时保留来源限制，不因模板格式变化而清掉绑定。

重命名 `Candidate Quality` 时，先部署新 workflow 并证明新 context，再迁移远端。新增业务检查接入聚合组件；新增多人 approval、CODEOWNERS 或 `dev` 保护需要重新评估实际评审安排。套餐不支持规则时报告限制，不宣称已强制。

参考 [GitHub 可用规则](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets)、[管理 / 导入 Rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/managing-rulesets-for-a-repository)与 [Rules REST API](https://docs.github.com/en/rest/repos/rules)。
