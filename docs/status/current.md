# 当前状态

更新日期：2026-10-13（Asia/Shanghai）。

## 当前结论

项目进入 M0 的规格复核与风险验证准备阶段。M0-A 本批已交付首笔本地记账与关键状态的文字规格、单一来源合成账务材料和 Android / iOS 核心验证矩阵；产品与仓库治理基线保持有效。

没有 Flutter 应用、Rust / Go 业务实现、数据库 schema、可执行部署模板或已上线官方端点；可点击原型、两端核心接入、加密与真机实验尚未开始。M0 尚未达到退出条件，规格与合成材料复算不能证明客户端、密码方案或同步已经通过。

仓库位于 `~/Code/RadishLedger`，本地开发分支为 `dev`，产品规划、协作治理与 UI 规格基线已通过首次本地提交保存。`origin` 已连接 [laugh0608/RadishLedger](https://github.com/laugh0608/RadishLedger)，首次基线已同步到远端 `master` / `dev`，默认分支为 `master`。未发布包或 tag；初始 `version.json` 只作为规划版本基线，不代表已有稳定产品版本。

## 已完成

- 确认 RadishLedger（萝卜账簿）名称与三种使用模式。
- 将公开 MVP 收敛为单人、单账簿、CNY，定义 R01–R12 能力与 T01–T23 验收。
- 形成账务口径、原子转账、普通退款、差额对账与合成数值基线。
- 形成 Rust 核心方向、同步 / 域隔离、密钥 / 恢复、备份 / 迁移和服务运维规格。
- 建立家族视觉方向、开发治理、根协作约定与个人免费自部署许可。
- 按私人记账 UI 的一般布局经验重写移动端 UI 规范，采用 family-ui `v26.7.3` 的 Workbench 与本地主题映射；补齐页面、金额 / 图表、亮暗、排版、交互与私有参考排除规则，形成 ADR 0003。尚无视觉稿或 Flutter 实现。
- 参考五个兄弟项目并独立形成协作与仓库治理、分支 / PR / 回流规则、基础属性、安全 / 行为规范、Issue / PR 模板与 ADR 0002。
- 专门逐份复核五个兄弟项目的十份 AGENTS / CLAUDE，补齐根入口的通用执行、资源生命周期、工程组织、材料来源、验证与交接约束，并同步协作专题和开发规范。
- 建立 Python 标准库仓库检查、定向入口和合成负向回归；提供 `PR Checks` / `Candidate Quality`、仓库目标设置及 `master` Ruleset 模板。
- 按明确授权连接公开空仓库，首次从已检查的 `f6f5b2c` 基线建立远端 `master` / `dev`；应用六项仓库设置，开启 merge / rebase，关闭 squash、自动合并和自动删分支。初始化 PR #1 已合入并回流；真实手动 Actions 检查成功。
- 启用 [master Ruleset 24995092](https://github.com/laugh0608/RadishLedger/rules/24995092)，精确匹配 `master`，要求 PR、会话解决和严格 `Candidate Quality`，绑定实际 GitHub Actions App `15368`，阻止删除与强推；审批基数为 `0`，管理员 bypass 限于 PR。`dev` 有效规则为空。
- 项目所有者完成仓库实际 Actions 启用后，新的 PR synchronize 事件自动触发 `PR Checks`，`Repo Hygiene` / `Candidate Quality` 均成功；首次引导补齐实际启用与自动事件验证，先前触发阻塞已解除。
- PR #2 最终候选自动检查成功，门禁为 `CLEAN` 且无未解决评审会话；已正常 merge 到 `master`（`fcb96f6`），Ruleset 执行结果为 `pass`，随后快进回流并同步 `dev`。未使用 bypass。
- 按 M0-A 顺位细化纯本地首笔流程、无账户 / 无流水空态、提交未定核对、解锁、冲突与独立恢复的正常 / 取消 / 失败路径；家族视觉与私人参考边界不变。
- 建立[共同账务材料](../validation/accounting-cases.md)：10 个独立案例、34 个检查点，以及 18 个金额、8 个日期和 5 个无效意图。输入与人工预期只维护在一份 JSON，承接原领域基线，并完成独立整数 / 日历复核。
- 建立[两端核心验证矩阵](../development/mobile-core-validation.md)，明确 bridge 比较、release 包、事务 / outbox、SQLCipher 文件边界、系统密钥失效、独立本机恢复、KDF 与证据 / 清理要求；没有预先锁定版本或修改加密方案。

文档建立过程与验证记录见[本月 devlog](../devlogs/2026-10.md)，治理规则见[治理导航](../governance/README.md)。

## 最近验证

M0-A 本批 `./scripts/check-repo.sh` 覆盖 50 个实际仓库文件及原有 22 条检查器回归，通过；文档链接、JSON、文本卫生、根入口一致及 `git diff --check` 通过。独立整数 / 日历复核确认共同材料的 10 个案例、34 个检查点、18 个金额输入、8 个日期和 5 个无效意图符合手写预期；这是样例复算，没有运行业务实现。验证记录见[本月 M0-A devlog](../devlogs/2026-10.md#2026-10-13-m0-a-账务与-ux-规格复核)。

治理基线已完成仓库检查与原有 20 条合成回归，Shell 语法与定向入口通过；曾在隔离临时仓库验证完整 PR base / head 范围、带空格路径的脚本入口、merge 晋级快进回流与分叉普通 merge，这些临时仓库证据不代表本项目已经完成真实 GitHub PR 或远端回流。UI 本批检查说明与实际证据见本月 devlog。

UI 本批仓库检查覆盖 46 个文件和原有 20 条回归，全部通过；14 组亮暗主题值与家族 tokens 核对，基础正文 / 次文本 / 主按钮色对计算通过。图片内容、原图文件名和原目录路径排除检查通过，私有参考目录的忽略与源码归档排除匹配通过；这些证据不替代视觉稿或真机验收。

`AGENTS.md` / `CLAUDE.md` 逐字一致。UI 规格已更新，其余产品范围、领域、技术、安全、许可和版本来源未改；私人截图未进入仓库或 Git，仅保留不含个人内容的文字规则。首次本地提交前和远端引导前的仓库检查均覆盖 46 个文件与 20 条检查器回归，全部通过；提交使用当前用户身份。

远端分支、默认分支与六项合并设置已由 API 读回；Actions 已开启。[初始化 PR #1](https://github.com/laugh0608/RadishLedger/pull/1) 已用 merge commit `ffeb0e9` 合入并快进回流 `dev`，仅作为保护启用前的首次引导。单一 JSON 来源生成的常规 YAML 通过本地 47 个文件 / 22 条回归检查，纳入默认分支后成功登记 `PR Checks`（workflow ID `383132006`）。[首次手动检查](https://github.com/laugh0608/RadishLedger/actions/runs/38389977556)的两项 job 均成功；来源为 GitHub Actions（App `15368`，owner `github`），模板与远端 required context 均绑定该来源。

Ruleset 详情与有效规则 API 读回为 active，`master` 命中删除、non-fast-forward、PR 和严格必需检查四类规则，`dev` 无有效规则。项目所有者确认此前仓库实际 Actions 功能未开启，现已在 GitHub 完成开启；先前仅凭 API 标记与手动成功的启用判断不充分。[首次自动 PR 运行 38395115506](https://github.com/laugh0608/RadishLedger/actions/runs/38395115506) 与[最终候选运行 38395969356](https://github.com/laugh0608/RadishLedger/actions/runs/38395969356) 均成功；最终 head `825c3dc` 的两项最新检查来自 Actions App `15368`，门禁为 `CLEAN`，评审会话数为 `0`。[治理 PR #2](https://github.com/laugh0608/RadishLedger/pull/2) 已正常 merge 为 `fcb96f6`，master rule suite `4498373031` 的结果为 `pass`；两父提交和内容与已检查候选一致，`dev` 已快进回流并同步。direct push / force push / deletion 风险写入未探测。业务、视觉稿、真机、部署与安全评审仍未验证。执行覆盖见[检查说明](../../scripts/README.md)。

## 下一批建议

仓库治理首次引导、自动 PR 检查和受保护正常合并 / 回流均已取得实际证据，自动触发阻塞已收口。

M0-A 本批规格材料已完成，下一项建议为 M0-B 两端最小核心接入，按[矩阵](../development/mobile-core-validation.md)推进：

1. 只读盘点本机 Flutter / Dart、Rust、Android 工具链、Xcode 与可用设备，核对官方版本条件及依赖许可；列出缺口、下载 / 安装影响和候选最低系统。
2. 后续实施任务明确开始后，使用共同材料比较薄 FFI 与 flutter_rust_bridge，完成两端 release 包的最小真实核心调用；同步接入实际语言检查，不使用返回成功的占位门禁。
3. 验证提交结果核对、事实 / outbox 原子性、重复意图、取消 / 重启与错误映射；衔接 M0-C 的加密库和系统密钥实验，以真实证据冻结 bridge、依赖、最低系统和密钥配置。
4. 首批合成设计稿随后核对现有文字状态与家族视觉，记录大字号、键盘、安全区和金额隐藏缺口；录入效率目标由真实交互验证，不因本批文字交付记为通过。

同步事务的提交顺序、幂等、代次、删除、迁移冻结留给 M0-D 的详细规格和故障验证；归档 header / manifest、资源边界、CSV 和暂存切换留给 M0-E。本批已复核相关 UX 与数值不变量，但没有冻结其生产格式。后续实施明确开始前，仍不引入业务工程、依赖或设备写入。

## 待冻结

| 项目 | 冻结依据 |
| --- | --- |
| bridge 与具体依赖版本 | 两端 release 打包与 FFI 行为 |
| 最低系统与机型矩阵 | 框架 / native 依赖及真机验证 |
| SQLCipher 与密钥保护配置 | 文件 / WAL / 临时文件、系统密钥失效 |
| KDF profile 与资源上下界 | 移动端耗时、内存、强度和错误行为 |
| HTTP / envelope / schema / ABI | 事务、共同样例、兼容与负向验证 |
| CSV / 归档格式与 limits | 保真、公式安全、认证与暂存恢复 |
| 服务资源、配额、保留期与官方地址 | 实际运维、压测和隐私评审 |

## 当前不做与停止线

- 不编写业务代码、安装业务依赖、启动服务或建立运行脚手架；仓库文档 / 配置检查属于已授权治理范围。
- 不导入真实账单，不操作用户密钥链或生产环境；私人 UI 参考只按明确授权本地观察，不做个人金融分析或复制 / 外发材料。
- 本批远端授权仅用于仓库连接、初始分支、治理设置、真实 PR / CI 验证与必要回流；不创建 tag / Release、发布、部署或更新 RadishX / 兄弟仓库。
- 不把本机文档或架构图表述为已经实现 / 安全评审 / 真机通过。
- 不跳过 M0 接入风险验证直接铺大量页面；不把 M1 称作完整公开 MVP。
