# 仓库检查

仓库治理入口使用 Python 3.11+ 标准库和 Git，不安装依赖、不访问网络、不修改文件或暂存区。Shell 入口适用于 macOS / Linux；其他环境可以直接运行 Python 入口。显式生成命令 `--write-workflow` 单独用于更新下述生成文件，不属于只读检查。Rust / Flutter 入口会使用构建缓存并在缓存缺失时下载锁定依赖，不能套用治理入口的无网络说明。

```bash
./scripts/check-repo.sh
./scripts/check-docs.sh
./scripts/check-text-files.sh
python3 -B scripts/check-repo.py

# PR：工作树为合并候选，提交范围为真实 head 相对 base 的共同祖先。
./scripts/check-repo.sh --base-ref BASE_SHA --head-ref HEAD_SHA
```

`--base-ref` 与 `--head-ref` 必须指向本地已有 commit，检查器不会 fetch。未给 head 时使用 `HEAD`；单独给 head 是错误。默认无 ref 模式可检查没有初始提交的工作区，不宣称已检查提交历史。

## 覆盖范围

| 入口 | 实际检查 |
| --- | --- |
| `check-text-files` | Git 已跟踪和未忽略的未跟踪文件，UTF-8、无 BOM、LF、末尾换行、尾随空白、JSON / Issue JSON-compatible YAML 解析、重复键、生成 workflow 与来源逐字一致 |
| `check-docs` | 上述文本检查，加 Markdown 本地链接 / 锚点、代码围栏、表格列数、根协作入口一致 |
| `check-repo` | 上述检查，加必需资产、版本格式、远端目标 / Ruleset / workflow / Issue 配置契约、强制跟踪忽略产物的拒绝、无门禁语言实现阻断、实际 Git diff 空白与可选 PR 提交范围；运行检查器自身的负向回归 |
| `check-core` | Rust fmt、锁定 workspace all-features test（账务、并发、进程恢复、密码向量与 bootstrap）、all-targets clippy、C header 的 clang 语法检查；不执行移动设备构建 |
| `check-mobile` | 锁定 pub、Dart format、fatal-infos analyze、Flutter test；实际加载宿主 Rust / SQLCipher，另验证 UI 错误 / 取消 / 晚结果与平台字节缓冲区；不验证真实系统认证 |

Markdown 允许非空行末的两个空格作硬换行；其他尾随空格仍报错。文件链接必须位于仓库并指向会进入 Git 的材料，避免链接只在维护者机器上存在。代码围栏里的命令和占位路径不作真实文档链接。外部 URL 不联网检查。

workflow 的单一手工来源为 [.github/workflow-sources/pr-check.json](../.github/workflow-sources/pr-check.json)，正式规则直接检查其 JSON 对象；[pr-check.yml](../.github/workflows/pr-check.yml) 为生成的常规 YAML，供 GitHub 读取。修改来源后显式生成，再运行检查：

```bash
python3 -B scripts/check-repo.py --write-workflow
./scripts/check-repo.sh
```

生成器只将 JSON 中的对象、数组、字符串、布尔、整数和 null 输出为 YAML，不解析用户提供的 YAML。所有检查入口逐字比较生成结果，手改生成文件、漏生成或多加 YAML 条目都会失败；改变正式来源即使重新生成，也必须继续通过原有触发、权限和聚合契约。这样可使用标准库完整验证来源及交付文件，无需用宽松正则猜 YAML 配置。

Issue 表单继续使用 JSON-compatible YAML，按 JSON 完整解析。其他任意 YAML 不宣称通过语法校验；新增自由编辑的 YAML 配置时，先选择正式解析依赖并更新检查器、CI 与回归。

仓库治理检查器不作秘密扫描、法律审查、账务核算或业务测试。业务结果由上面的实际语言入口验证。未跟踪的忽略目录不扫描；已跟踪文件不会因 `.gitignore` 而跳过，强制跟踪的忽略产物会报错。未知二进制文件不按 UTF-8 处理，已知文本扩展名中的 NUL / 非 UTF-8 仍失败。正式材料单文件超过 5 MiB 时失败，较大资产需先说明来源和实际交付用途再调整规则。

## 自动化契约与扩展

`PR Checks` 在合并候选执行 `check-repo`，提交检查不把 GitHub 合成 merge commit 当作用户提交。真实 merge commit 根据父提交数量豁免 Conventional Commits；单父提交即使以 `Merge` 开头也不能绕过检查。PR 范围拒绝已知 AI 的 `Co-authored-by` 署名。

workflow 的聚合必须 `always()` 等待 `Repo Hygiene` 与 `Core Quality`，只接受全体 `success`。后者使用 `rust-toolchain.toml` 的 Rust 和 [setup-flutter-ci.py](setup-flutter-ci.py) 的固定 Flutter revision，复用两个真实语言入口。设置脚本只接受 GitHub Actions 环境，在 RUNNER_TEMP 准备 SDK，不用于修改开发机工具链。检查器验证聚合脚本、依赖、权限、凭据、Action pin 与触发契约；负向回归实际执行聚合脚本，证明任一组件失败、取消、跳过或缺失都会拒绝。

新增 workflow 必须同步审阅清单，避免额外 push / tag 触发或重复 required context 绕开声明的 CI 策略。

Action 使用 [actions/checkout v7.0.1](https://github.com/actions/checkout/releases/tag/v7.0.1) 的[完整 commit](https://github.com/actions/checkout/commit/3d3c42e5aac5ba805825da76410c181273ba90b1)。选择记录用于复核引用，不宣称已在远端运行；升级时审阅来源、权限和实际 PR 行为。

Rust / Dart 仅允许已接入真实检查的 `crates/ledger-core`、`crates/ledger-mobile` 和 `apps/m0_probe`；未知语言工程或 Go 实现仍会阻断。检查器同时核对必需 manifests / lockfile / 工具链和根版本镜像，语言组件不能增加跳过条件或改成返回成功的脚本。新增实际模块或语言时同步更新 workflow、检查器契约、说明和回归；更复杂的恢复、协议、数据库、平台与安全验证按[开发规范](../docs/development/README.md)执行。

## 移动构建、生成与证据

[build-mobile-core.py](build-mobile-core.py) 由 Dart build hook 调用，以真实目标编译同一个 Rust release code asset；不会安装工具、制造空库或回退宿主。Android clang / ar / ranlib 与 Apple SDK 显式传递。移动构建与实际 `flutter drive` 验证命令见[实验说明](../docs/development/mobile-core-prototype.md)，模拟器和真机证据分别记录。

[generate-mobile-bridge.py](generate-mobile-bridge.py) 使用项目内固定 FRB / cargo-expand，从 `crate::api` 生成 Rust / Dart binding，检查工具版本并关闭自动依赖升级。它会写生成代码，只在 API 改动后显式执行。

[collect-native-notices.py](collect-native-notices.py) 从本机锁定缓存收集两端 arm64 与验证宿主的 native 许可，并保留固定 Rust 标准库的官方许可集；`--check` 只读比较。它不联网，缓存缺失明确失败；重新生成需复核 [THIRD_PARTY_NOTICES](../THIRD_PARTY_NOTICES.md)。普通 CI 没有宣称完整供应链扫描。

M0-B 运行报告和合成截图写入 `local-evidence/m0-b/`，M0-C 写入 `local-evidence/m0-c/`；由各集成 driver 或设备控制器保存，构建产物和缓存保持忽略。同一 Flutter 工作目录的构建、测试与模式切换按实验说明串行执行。没有私人账单或私人参考衍生材料，不能改成截取用户已有设备 / 容器。

错误逐项报告并返回非零；检查器没有历史预算或默认成功模式。`--only` 用于定向本地检查，PR workflow 必须使用完整入口。


### M0-B 故障与跨进程恢复

`check-core.sh` 包含共享 SQLCipher 并发和独立子进程 SIGKILL 恢复检查；`check-mobile.sh` 包含实际两种 bridge 的有界压力、panic / ABI 错误及句柄归还检查。模拟器还需分别运行 `resilience_test.dart` 与 `check-mobile-recovery.py`，步骤、固定合成材料和清理边界见[移动核心实验](../docs/development/mobile-core-prototype.md#6-压力故障并发与强杀恢复实验)。该 Python 入口只控制明确指定的现有模拟器内本项目 App，不启动 / 新建设备；它需要可用 adb / simctl 权限，真实结果保存于忽略目录。普通宿主检查不能替代移动端证据。


### M0-C bootstrap 与独立解锁

[check-mobile-bootstrap.py](check-mobile-bootstrap.py) 经 App 私有 FIFO 传递有界秘密字节，复用 [mobile_probe_device.py](mobile_probe_device.py) 的现有模拟器控制；该共享模块同时供 M0-B 恢复入口使用。两种 bridge 验证创建确认、独立材料跨 PID 解锁、错误输入保全、六处中断、A11 非空 WAL 强杀 / 原意图重放及精确清理；每端共 16 组恢复和 48 组封装文件故障。`--output-dir` 可保留独立批次，不能离开项目 M0-C 证据目录或覆盖仍登记 ownedRuns 的报告。系统认证能力不足会单独记录未执行，不降低保护；核心 local-wrap 不作为系统认证证据。入口、安装模式、UI 验证与失败残留处理见[移动核心实验第 7 节](../docs/development/mobile-core-prototype.md#77-已接入的复现入口与证据边界)。本入口只操作合成数据，不接受真实账库。

`check-core.sh` 同时执行 A11 非空候选 / 删除 / outbox 跨进程恢复、原意图重放、四类封装文件发布故障，以及实际 DB / WAL / 回滚日志检查；`check-mobile.sh` 的 `bootstrap_native_test.dart` 经宿主两种 bridge 验证非空账务和发布错误。故障注入、发布结果未定及宿主 / 设备证据边界见[文件故障专题](../docs/development/mobile-core-prototype.md#78-非空账务与文件发布故障)。这不等于真实磁盘损坏、断电或系统认证已验收。
