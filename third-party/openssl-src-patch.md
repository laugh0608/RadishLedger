# openssl-src 完整来源与 ARM 能力补丁

[openssl-src](openssl-src/Cargo.toml) 是 crates.io 的 `400.0.2+4.0.3` 完整发布包副本，共 2492 个文件、约 33.16 MB，112 个可执行文件保留原模式。根 Cargo.toml 通过 `[patch.crates-io]` 选择该目录，并将它排除在 workspace 成员之外；继续使用上游构建逻辑和现有静态 OpenSSL provider。

[来源清单](openssl-src-upstream.json)保存归档 URL、SHA-256、上游提交、全部原始文件哈希、可执行文件及精确文本例外。归档 SHA-256 为 `d2093956673b8dbda85c87fa4fb78e70698ce6c5978a2837d1613f661d3bef76`。仅 `openssl/crypto/armcap.c` 应用[两行补丁](openssl-armcap-sve.patch)：先有 ARMV8_SVE，才接受 HWCAP2_SVE2。前后哈希与应用状态见[补丁元数据](openssl-armcap-sve.json)。其他文件保持原包字节，不格式化上游、不修改机器缓存、不引入下载器或构建 wrapper。

该条件拒绝 SVE=0 / SVE2=1 的矛盾组合，防止初始化执行不可用的 SVE 指令；有效 SVE2 路径及其他加速位保留。它不改变算法、KDF、密码格式、SQLCipher 或其他依赖。根 lockfile 相比正式接入前仅移除该包的 registry source / checksum，版本不变；完整来源校验由本地清单承担。配套的 [openssl-sys 最小版本约束补丁](openssl-sys-patch.md)仍单独核验。

上游 31 个文件存在末尾空白或缺少末尾 LF；Git 的空白检查另覆盖空白末行、空格后接 Tab 等情况，共需 77 个精确路径属性。`check-repo.py` 先校验清单自身的固定哈希，再按原始文件哈希允许指定文本例外，并核对全部源码、模式、补丁和 `.gitattributes` 路径集合。未知文件、修改后的例外文件、扩大例外或清单漂移均失败。UTF-8、NUL、大小及结构检查不因此关闭；没有目录通配例外，普通项目文件继续执行原规则。单独 `git diff --check` 不证明来源完整，必须通过仓库检查。根 AGENTS / CLAUDE 的已批准原字节例外规则保持逐字一致。

Flutter native hook 跟踪两份本地源包的全部文件，源码变更会使构建资产失效；它们不作为 App asset 打包。MIT / Apache-2.0 包装许可、OpenSSL Apache-2.0 及内含 Text::Template 构建工具许可原文均保留；native notices 显式收集嵌套材料，缺失或为空即失败。Text::Template 是构建期材料，不是新增运行时密码提供方。

源码能力诊断分别编译原始 / 修补后 armcap.c，运行 SVE / SVE2 四组合；实际 Android provider、算法、pager 与 App 回归不设置能力覆盖。验证范围与当前结果见[移动核心专题](../docs/development/mobile-core-prototype.md#32-openssl-修补方案评审与验证计划)。模拟器通过不能代替真实 SVE2 硬件、系统认证或真机文件保护。

后续正式上游包包含等价修复、并通过来源 / 许可、旧库恢复和目标回归后，撤除本地 openssl-src patch、副本、专用清单及例外；sys 版本约束补丁按其独立撤除条件处理。额外 C / 汇编修改、算法或 provider 改动、其他依赖升级须另行评审，不扩展本次两行修复。升级验证不等于新版写库可降级，不能以二进制回退代替数据恢复验证。
