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

Rust / Dart 仅允许已接入真实检查的 `crates/ledger-core`、`crates/ledger-mobile` 和 `apps/m0_probe`；另有明确排除 workspace 的 `third-party/openssl-sys` 与 `third-party/openssl-src` 完整上游副本，由原包哈希 / 最小差异检查覆盖并经 Linux core-quality 实际编译；未知语言工程或 Go 实现仍会阻断。检查器同时核对必需 manifests / lockfile / 工具链和根版本镜像，语言组件不能增加跳过条件或改成返回成功的脚本。新增实际模块或语言时同步更新 workflow、检查器契约、说明和回归；更复杂的恢复、协议、数据库、平台与安全验证按[开发规范](../docs/development/README.md)执行。

## 移动构建、生成与证据

[build-mobile-core.py](build-mobile-core.py) 由 Dart build hook 调用，以真实目标编译同一个 Rust release code asset；不会安装工具、制造空库或回退宿主。Android clang / ar / ranlib 与 Apple SDK 显式传递。移动构建与实际 `flutter drive` 验证命令见[实验说明](../docs/development/mobile-core-prototype.md)，模拟器和真机证据分别记录。

[generate-mobile-bridge.py](generate-mobile-bridge.py) 使用项目内固定 FRB / cargo-expand，从 `crate::api` 生成 Rust / Dart binding，检查工具版本并关闭自动依赖升级。它会写生成代码，只在 API 改动后显式执行。

[collect-native-notices.py](collect-native-notices.py) 从本机锁定缓存收集两端 arm64 与验证宿主的 native 许可，并保留固定 Rust 标准库的官方许可集；`--check` 只读比较。它不联网，缓存缺失明确失败；重新生成需复核 [THIRD_PARTY_NOTICES](../THIRD_PARTY_NOTICES.md)。本地 openssl-sys / openssl-src 仅接受根 patch 指定的精确路径，保留包装与嵌套许可及补丁标记（包括构建期 Text::Template）；其他未知本地依赖不静默略过。普通 CI 没有宣称完整供应链扫描。

sqlcipher-src 的包装许可、嵌套 SQLCipher / libtomcrypt 许可与 SQLite header 首段声明分别收集；缺失、空文件或声明格式漂移均拒绝生成。[许可回归](tests/test_native_notices.py)使用临时合成包验证完整收集和拒绝路径，由 `check-repo` 的测试发现入口执行，不接触真实缓存。

SQLCipher 升级另需跨版本检查：在依赖修改前，从已知旧锁定版本构建 `m0-bootstrap-worker`，单独复制到本批忽略证据目录，并记录 Git / Cargo.lock / binary 指纹；不能把升级后重建的 worker 当作旧基线。完成升级后，显式执行：

```bash
M0_PREVIOUS_BOOTSTRAP_WORKER=/absolute/path/to/saved/baseline-worker \
  cargo test --release --locked -p radishledger-core --test bootstrap_process \
  previous_sqlcipher_preserves_closed_and_interrupted_vaults -- --ignored --nocapture
```

该用例核对旧 worker 为 SQLCipher 4.14.0、新 worker 为 4.19.0，验证正常关闭与强杀后待恢复 WAL；旧版仅读取后者副本，原文件由新版首次打开。A11 独立预期、错材料不改写、完整事实 / 身份、原意图重放与清理均断言。随机秘密仅经内存和管道传递，合成目录由测试清理；不接收用户账库。默认 ignored 是对外部旧二进制前置条件的明确要求，`check-core` 不将其计为通过；单独执行缺少旧 worker 时失败。此结果不承诺新写库回退兼容，也不代替移动设备运行。

OpenSSL 升级另用 `M0_PREVIOUS_OPENSSL_WORKER` 和 `previous_openssl_preserves_closed_and_interrupted_vaults`，其他 cargo 参数与上例相同。旧 worker 必须是 SQLCipher 4.19.0 + OpenSSL 3.6.3，候选为 SQLCipher 4.19.0 + OpenSSL 4.0.3；测试保存前后的 provider 运行日志及二进制指纹，不能以 Apple CommonCrypto 冒充（该用例在 Apple 明确拒绝执行）。两个用例复用同一 A11 保真 / 恢复流程，但外部 worker 来源必须分别证明。

[openssl_vectors.c](tests/openssl_vectors.c) 是独立公开密码向量和 RAND / SQLCipher 初始化失败传播验证，当前由迁移批次显式执行，不纳入普通 macOS CommonCrypto 检查。用目标 C 编译器加 `-std=c11 -Wall -Wextra -Werror -DSQLITE_HAS_CODEC`，显式传入同批 Cargo 输出的 OpenSSL / SQLCipher include、`libsqlcipher.a` 和 `libcrypto.a`，Linux 另加 `-ldl -pthread -lm`，Android 另加 `-ldl -lm -llog`；不得以系统 `-lcrypto` 代替。执行参数依次为 `vectors` 或 `provider-failure`、精确版本字符串、尚不存在的合成数据库路径。两个模式必须分别启动进程，版本 / 向量 / 预期失败不符返回非零，成功时只删除指定数据库。失败应先检查精确路径、保留诊断，再按归属清理，不全局覆盖 OpenSSL 配置。实际完成范围及历史阻塞见[OpenSSL 专题](../docs/development/mobile-core-prototype.md#32-openssl-修补方案评审与验证计划)。

[openssl_armcap_probe.c](tests/openssl_armcap_probe.c) 将实际 OpenSSL armcap.c 的 `getauxval` 改名为诊断替身（编译参数 `-Dgetauxval=probe_getauxval`），直接编译原始 / 候选源码，并显式传入同批生成的 OpenSSL include、源包 include 与 crypto 目录。按目标使用 `-Wall -Wextra -Werror`，分别以 `TEST_CAPS=00/01/10/11` 启动进程；无效输入退出 2。独立期望表要求无 SVE 时不能启用 SVE2，检查其他密码加速位及向量长度探测次数。上游 4.0.3 的 01 组合预期失败，候选四组预期通过。该替身仅属于单元诊断；普通 provider / pager / App 测试不得携带 TEST_CAPS 或 OPENSSL_armcap 能力覆盖。可审阅补丁及其前后指纹见[ARM 修复专题](../docs/development/mobile-core-prototype.md#arm-能力修复的隔离验证与正式接入方案)。主工作区已选择含此补丁的完整源包，来源 / 精确文本例外与撤除条件见[源码说明](../third-party/openssl-src-patch.md)。

M0-B 运行报告和合成截图写入 `local-evidence/m0-b/`，M0-C 写入 `local-evidence/m0-c/`；由各集成 driver 或设备控制器保存，构建产物和缓存保持忽略。bootstrap UI driver 支持 `M0C_EVIDENCE_SUBDIR`，resilience driver 支持 `M0B_EVIDENCE_SUBDIR`；两者只接受单个英文字母 / 数字 / 下划线 / 连字符名称，以保留历史批次。同一 Flutter 工作目录的构建、测试与模式切换按实验说明串行执行。没有私人账单或私人参考衍生材料，不能改成截取用户已有设备 / 容器。

错误逐项报告并返回非零；检查器没有历史预算或默认成功模式。`--only` 用于定向本地检查，PR workflow 必须使用完整入口。


### M0-B 故障与跨进程恢复

`check-core.sh` 包含共享 SQLCipher 并发和独立子进程 SIGKILL 恢复检查；`check-mobile.sh` 包含实际两种 bridge 的有界压力、panic / ABI 错误及句柄归还检查。模拟器还需分别运行 `resilience_test.dart` 与 `check-mobile-recovery.py`，步骤、固定合成材料和清理边界见[移动核心实验](../docs/development/mobile-core-prototype.md#6-压力故障并发与强杀恢复实验)。该 Python 入口只控制明确指定的现有模拟器内本项目 App，不启动 / 新建设备；它需要可用 adb / simctl 权限，真实结果保存于忽略目录。普通宿主检查不能替代移动端证据。


### M0-C bootstrap 与独立解锁

[check-mobile-bootstrap.py](check-mobile-bootstrap.py) 经 App 私有 FIFO 传递有界秘密字节，复用 [mobile_probe_device.py](mobile_probe_device.py) 的现有模拟器控制；该共享模块同时供 M0-B 恢复入口使用。两种 bridge 验证创建确认、独立材料跨 PID 解锁、错误输入保全、六处中断、A11 非空 WAL 强杀 / 原意图重放及精确清理；每端共 16 组恢复和 48 组封装文件故障。`--output-dir` 可保留独立批次，不能离开项目 M0-C 证据目录或覆盖仍登记 ownedRuns 的报告。系统认证能力不足会单独记录未执行，不降低保护；核心 local-wrap 不作为系统认证证据。入口、安装模式、UI 验证与失败残留处理见[移动核心实验第 7 节](../docs/development/mobile-core-prototype.md#77-已接入的复现入口与证据边界)。本入口只操作合成数据，不接受真实账库。

`check-core.sh` 同时执行 A11 非空候选 / 删除 / outbox 跨进程恢复、原意图重放、四类封装文件发布故障，以及实际 DB / WAL / 回滚日志检查；`check-mobile.sh` 的 `bootstrap_native_test.dart` 经宿主两种 bridge 验证非空账务和发布错误。故障注入、发布结果未定及宿主 / 设备证据边界见[文件故障专题](../docs/development/mobile-core-prototype.md#78-非空账务与文件发布故障)。这不等于真实磁盘损坏、断电或系统认证已验收。

`bootstrap_experiment_test.dart` 经两种真实 bridge / 核心验证人工实验管理：精确项缺失后的 A11 独立恢复、重建、全部候选清理登记、失败 / 取消重试、核心目录删除后的继续清理与无关对象保留；平台系统保护为内存替身。`bootstrap_widget_test.dart` 覆盖确认、后台取消等待、清理失败后重开页及清理成功后切换 bridge，宿主页面重建不算设备重启。正常测试发现入口包含这两份测试；操作和登记边界见[人工实验流程](../docs/development/mobile-core-prototype.md#713-人工实验页的精确失效清理与候选切换)。

[collect-bootstrap-evidence.py](collect-bootstrap-evidence.py) 只离线重组人工页显式输出的本批合成分片，不启动 adb / simctl、不安装或连接设备。输入 / 输出限本仓库 `local-evidence/m0-c`，已保存的普通 UTF-8 输入上限 2 MiB、报告上限 96 KiB；缺片、冲突、身份不符或输出已存在均拒绝，错误不回显输入内容。它不执行 B / C 验收，也不把任意日志清洗为可公开材料。字段来源、批次开关、精确命令和局限见[有界非秘密报告](../docs/development/mobile-core-prototype.md#714-人工实验的有界非秘密报告)。

`bootstrap_lock_review_test.dart` 经 thin / FRB 的真实 Rust / SQLCipher 验证同库非秘密基线、错材料保全、重新独立解锁 / 系统适配、取消后复核与清理重试；系统认证和文件观测是显式替身。`bootstrap_lock_review_widget_test.dart` 覆盖后台输入清除、窗口继续 / 取消、重新解锁失败、摘要不符、晚结果抑制、重开页缺失基线和窄屏两倍字号。两份测试纳入正常 Flutter 发现入口；[同库复核契约](../docs/development/mobile-core-prototype.md#716-解锁后-a11-前后复核)区分文件保护观测和账务比较，不能据宿主结果宣布真机验收。

`bootstrap_evidence_test.dart` 测试报告字段排除、记录边界、默认禁止输出、停止等待 / 晚成功、失败重试、同库复核投影 / 96 KiB 上限及 Dart → Python 实际往返，后者使用 PATH 中已有 Python 标准库。`scripts/tests/test_bootstrap_evidence.py` 的重组 / 路径 / 文件边界回归由仓库检查自动发现；两组均不读取真实设备日志。共用分片函数维持原 App 文件报告的格式，并由宿主测试核对。

Unix 宿主的 `check-core.sh` 还执行 `store::pager_tests`：独占子进程中的测试 VFS 在实际 WAL 提交、DELETE 提交和 checkpoint 的写入 / 同步位置注入故障，SIGKILL 后由未注入的普通入口恢复并重试；另观察 TEMP 表、排序 / 索引、物化查询、语句回滚、VACUUM、super-journal 和 SHM。MEMORY 策略与 FILE 正向对照分别断言，匿名临时文件在关闭前扫描。可通过 `M0_PAGER_EVIDENCE_DIR` 保存忽略目录内的有界 JSON 证据；子进程入口标记 ignored，由两个父测试实际调度，不能作为未运行或单独通过的业务测试计数。完整判据、清理和适用边界见[DB pager 与临时文件](../docs/development/mobile-core-prototype.md#79-db-pager-故障与临时文件边界)。


`build-mobile-core.py --pager-tests` 用既有目标配置生成独立的 `cfg(test)` 原生核心可执行文件，不改变 App code asset。新增 [check-mobile-pager.py](check-mobile-pager.py) 复用已有模拟器 / App 定位能力，先核对实际目标、默认 VFS 与全部编译选项，再运行相同 pager / 临时文件 harness，并要求所有子进程配置一致。Android `TEMP_STORE=3` 强制内存，FILE 请求仍须不落盘；iOS `TEMP_STORE=2` 执行真实 FILE 临时文件对照，两端另有独立明文扫描对照。配置、运行与精确清理见[移动原生复验](../docs/development/mobile-core-prototype.md#710-移动模拟器原生-pager-复验)；不自动启动设备、安装 App、改变锁屏或证明 Flutter / 生产安全域 / 真机行为。

[check-mobile-app-files.py](check-mobile-app-files.py) 控制已安装专用入口的现有模拟器 App，不自行构建 / 安装或启动设备。正常 App 进程运行两 bridge × MEMORY / FILE，独立复核路径、UID、权限、Android release / SELinux / shell 拒绝及 iOS 备份排除；`--interrupt-once` 实际停止后要求新 PID 清理登记并重新完成。iOS Simulator 缺少文件保护属性记录未验证，不默认成功。专用入口、诊断传输和精确恢复步骤见 [App 文件复验](../docs/development/mobile-core-prototype.md#711-app-进程安全域与文件路径复验)。`check-mobile.sh` 的 `file_boundary_test.dart` 验证宿主真实双 bridge 以及弱保护、调试进程和越界路径拒绝；不代替移动证据。

[check-ios-file-protection.sh](check-ios-file-protection.sh) 用现有 macOS / Xcode Swift 工具链编译实际 iOS 文件采样器；运行 `sh scripts/check-ios-file-protection.sh` 验证 DB / WAL / SHM 合成内容保留、身份 / 链接拒绝、同库缺失与零长度 sidecar 的独立覆盖、独立对照实际 pwrite / fsync / 回读、已有对照不覆盖与描述符关闭，以及注入状态下的超时、取消、短写 / 同步失败和错误分类。该独立定向检查不连接设备或模拟硬件保护，临时目录与编译缓存退出时清理；可选 App 入口、45 秒窗口及证据边界见[有界锁屏观测](../docs/development/mobile-core-prototype.md#715-ios-有界锁屏访问观测)。
