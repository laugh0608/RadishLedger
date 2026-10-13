# 仓库、分支与 PR 治理

本文是 RadishLedger 的 Git 与仓库治理来源，选择理由见 [ADR 0002](../adr/0002-repository-governance.md)。远端是否建立、规则是否启用以及最近证据只在[当前状态](../status/current.md)维护。

## 1. 基础资产

| 资产 | 职责 |
| --- | --- |
| `README.md` / `docs/README.md` | 产品定位与文档导航 |
| `AGENTS.md` / `CLAUDE.md` | 一致的启动协作约束 |
| `CONTRIBUTING.md` / `CODE_OF_CONDUCT.md` | 贡献、讨论、审查与处理渠道 |
| `LICENSE` / `SECURITY.md` | 软件授权与私下安全报告 |
| `.editorconfig` / `.gitattributes` | 编码、缩进、换行、二进制与源码归档属性 |
| `.gitignore` | 本地配置、数据、秘密、证据和生成物边界 |
| `version.json` | 手工产品版本来源；其他版本身份按开发规范独立管理 |
| `scripts/` | 本地与 CI 共用的仓库检查 |
| `.github/ISSUE_TEMPLATE/` / PR 模板 | 合成复现、需求影响与验证证据 |
| `.github/workflow-sources/pr-check.json` / `.github/workflows/pr-check.yml` | 单一手工检查来源与生成的 GitHub workflow |
| `.github/repository-settings.json` / `rulesets/` | 可审阅的远端目标设置与启用说明 |

忽略规则不是安全扫描，也不能隐藏已跟踪的秘密；发现泄露应按安全报告处理，不以删除工作区文件声称凭据已撤销。`git archive` 排除本地数据 / 证据是额外保护，不替代发行包内容检查。

## 2. 分支角色

| 分支 | 用途与进入方式 |
| --- | --- |
| `dev` | 日常开发与集成；普通串行任务直接提交，按风险本地验证 |
| `master` | 远端默认稳定主线；经 PR 接收阶段晋级或必要 hotfix |
| 按需主题分支 | 外部贡献、并行写入、隔离实验或明确评审需求，通常 PR 进入 `dev` |
| `hotfix/*` | 从稳定主线修复必须立即处理的问题，经 PR 进入 `master` 后回流 |

主题分支可用 `feature/`、`fix/`、`docs/`、`chore/`、`experiment/` 或 `hotfix/`；Agent 需要创建时默认 `codex/`。实际命名服从用户要求，不为普通任务自动建分支或 worktree。

单维护者基线不为 `dev` 配置强制 PR、required checks 或 Ruleset；它仍受共享历史、授权和本地验证约束。出现稳定多人维护、持续外部贡献或实际绕过验证问题时，重新评估保护范围。

## 3. 提交、PR 与合并

提交使用当前用户 Git 身份和 Conventional Commits，例如 `docs(governance): 建立仓库治理基线`。复杂改动补充简短正文，破坏性变更说明兼容与迁移影响。只暂存精确路径，不用 `git add .` 混入用户或并行文件；不添加 AI 署名，不自行 amend 或重写既有提交。

PR 需要说明具体问题、最终行为、范围、实际验证、未验证内容、数据影响与恢复方式。目标为 `dev` 的 PR 提供评审与 CI 反馈；稳定晋级的门禁收口到 `master`。review conversation 必须解决，单维护者不要求额外 approval；出现真实独立评审安排后再提高审批数和考虑 CODEOWNERS。

| PR 类型 | 合并方式 |
| --- | --- |
| 常态 `dev -> master` 晋级 | 优先 merge commit，保留共享分支拓扑 |
| 主题分支 / hotfix | merge commit 或 rebase merge，结合历史和回流成本选择 |
| 所有 PR | 禁用 squash merge |

仓库与 Ruleset 都允许 `merge` / `rebase`；这不授权操作者 rebase 共享 `dev`。GitHub rebase merge 会生成新 SHA，回流需普通 merge。线性历史规则与保留 merge commit 冲突，因此不启用。合并方法的实际行为见 [GitHub PR merges](https://docs.github.com/en/pull-requests/reference/pull-request-merges)。

针对 RadishLedger，下列变更必须补充专门证据：

| 变化 | 必须审查的影响 |
| --- | --- |
| 金额、账户、转账、退款、余额或报表 | 整数精度、事实 / 派生边界、原子性和共同数值样例 |
| 存储、schema、备份、迁移与升级 | 未同步事实保留、事务失败、重启和独立恢复 |
| 同步、冲突、删除和端点 | 幂等、条件提交、候选保留、防复活、域隔离和兼容 |
| 加密、密钥、设备、解锁和恢复 | 威胁边界、标准向量、失败关闭、密钥失效与元数据暴露 |
| Flutter / Rust FFI | 契约版本、编码、所有权、释放、线程、取消和错误语义 |
| 自部署 / 官方服务 | 相同核心协议、实例与账库隔离、升级 / 回滚和目标证据 |
| 依赖、资产、许可与发布 | 来源、版本、notices、分发权和签名 / 包内容 |

CI 检查实现满足声明，不替代对账务规则、协议或安全方案本身的评审。

## 4. 合并后回流

每次晋级或 hotfix PR 合入 `master` 后，下一轮开发前把最新稳定主线合回 `dev`。执行前确认工作区干净、分支与远端目标正确，并具有 fetch / push 对应授权。

```bash
git fetch origin
git switch dev
git merge --ff-only origin/dev
git merge --ff-only origin/master
git push origin dev
```

若 `origin/dev` 与本地已分叉，先查看分支图、保护本地提交并正常合并，不盲目继续。如果第二个快进因 rebase merge 或后续提交失败，审查拓扑后使用：

```bash
git merge --no-ff origin/master
# 解决冲突并验证实际内容变化后，再按授权推送 dev。
```

回流不使用 reset、共享分支 rebase 或 force push。只改变拓扑、文件树未变化且原候选已通过门禁时，不重复全量验证；冲突解决和任何实际内容变化先做相关检查。回流不自动触发 tag、Release、部署或版本递增。

## 5. 远端目标设置

`.github/repository-settings.json` 保存可用于仓库 REST 更新的目标字段：默认分支 `master`，允许 merge / rebase，关闭 squash、自动合并和合并后自动删分支。保留分支是为防止长期 `dev` 被自动删除；主题分支在确认已合并、无未推送提交且获得清理授权后按需删除。

`master-protection.json` 目标为 active branch Ruleset：

| 项目 | 目标 |
| --- | --- |
| 分支匹配 | 精确 `refs/heads/master` |
| 删除 / non-fast-forward | 阻断 |
| PR | 必须；解决全部 review conversation |
| approval / CODEOWNERS / last-push approval | 单维护者为 `0` / 不要求 / 不要求 |
| 合并方式 | `merge`、`rebase` |
| required check | 仅 `Candidate Quality`，strict / up-to-date |
| 管理员 bypass | 仅 PR 内；仅用于已授权的例外，记录原因和缺失证据 |
| 创建分支检查例外 | 关闭；保护启用前完成分支与检查引导 |
| 线性历史、签名提交、commit-message 正则 | 不启用；提交格式由 PR 范围本地检查 |

PR 内 bypass 仍可绕过检查，所以不是“管理员也绝对不能合并失败候选”；不得将它作为日常合并路径。只有正式例外授权才使用。GitHub Ruleset 由远端强制，JSON 存在不代表已经部署。规则可用性取决于仓库可见性和 GitHub 套餐，必须在目标仓库核对。参见 [Ruleset 可用规则](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets)。

不预建 tag / Release 保护或自动部署。存在真实发行载体、签名、支持矩阵和恢复证据后，再设计对应策略。

## 6. CI 与门禁演进

`PR Checks` 只由目标为 `dev` / `master` 的 `pull_request` 和 `workflow_dispatch` 触发。普通 push 与 tag 不触发；不使用 `pull_request_target` 执行贡献代码。使用只读 contents 权限、关闭 checkout 凭据持久化、Action 固定完整 commit SHA，并为任务设置超时。

`Repo Hygiene` 执行仓库检查与检查器自身的负向回归；`Candidate Quality` 用 `always()` 汇总全部组件，仅当所有组件都是 `success` 才通过。失败、取消、跳过和缺失不能转成成功。required context 是 job 名 `Candidate Quality`，不包含 workflow 前缀或事件后缀。

workflow 从 `.github/workflow-sources/pr-check.json` 显式生成常规 YAML；检查器同时约束来源语义与生成文件逐字一致，不手改 `.github/workflows/pr-check.yml`。生成命令、实际覆盖与格式限制见[检查说明](../../scripts/README.md)。

PR 文件检查针对合并候选的工作树；提交检查用 `merge-base(base, head)..head` 校验实际 PR 提交，避免 fork 历史或 GitHub 合成 merge commit 干扰。SHA 经环境变量传入，不将 PR 标题、分支名或正文插入 shell。手动检查不假装有 PR commit range。

新增语言实现时必须将对应真实组件接入聚合依赖和结果判定；仓库检查会阻断没有门禁的首个 Rust / Go / Dart 源文件或 manifest。首次接入同步修改检查器的组件契约、workflow、文档和负向回归。账务、加密、迁移、安全与真机证据仍按开发规范的风险矩阵取得，不能从文档门禁推定。

required check 改名或拆分前，先让新 context 在真实候选产生并通过，再更新远端。治理变更同时复核脚本、模板、ADR 与实际远端设置，不能为通过 CI 放宽规则。
