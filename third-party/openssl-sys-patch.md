# openssl-sys 本地最小补丁

本目录旁的 [openssl-sys](openssl-sys/Cargo.toml) 是 crates.io 发布的 `openssl-sys 0.9.117` 完整 81 文件副本，MIT 许可保留在原包内。原包地址、SHA-256、上游提交与每个文件的原始哈希见 [来源清单](openssl-sys-upstream.json)。它仅服务现有 SQLCipher 非 Apple 静态 OpenSSL 构建，不是新的 workspace 成员。

唯一语义修改是 Cargo.toml 与 Cargo.toml.orig 中 openssl-src 构建依赖从 `300.2.0` 改为 `=400.0.2`，对应根 Cargo.lock 的 OpenSSL `4.0.3`；不更改 features、绑定、build.rs 或 C 源码。另为 `.cargo_vcs_info.json` 添加末尾 LF，满足仓库文本要求；其他字节保持原包一致，内附 Cargo.lock 作为上游发布材料保留，不参与根 workspace 解析。

原包已有 `Openssl4xx` / `ossl400` 分支；本补丁不关闭版本检查，不将 400 系列伪装为 300，也不改动机器缓存。根 Cargo.toml 的 `[patch.crates-io]` 明确选择该来源；同时选择经独立批准的 [openssl-src 完整源码与 ARM 补丁](openssl-src-patch.md)，不在本包扩大源码修改。许可生成器将其登记为本地 source-dependency patch，继续收集 MIT 正文，不能因 Cargo source 为 null 而遗漏。

`check-repo.py` 反向还原上述两个版本字段及 JSON 末尾 LF，逐文件核对原包哈希，并验证清单无增删、根 patch 路径与 workspace 排除项；该源码由 Linux core-quality 实际编译。上游文件不参与本项目 rustfmt 改写，不能为了格式检查扩大第三方差异。

当正式 openssl-sys 发布版直接支持所需 openssl-src 且完成同等来源、旧库、两桥接及目标回归后，撤除本地 patch、源码副本与专用完整性约束。若在此之前需要修改绑定实现或其他依赖，先停止并重新评审范围。源码变更不自动授权数据库降级；新版写库向旧版回退需独立验证。

采用背景、实际状态及验证边界见[移动核心专题](../docs/development/mobile-core-prototype.md#32-openssl-修补方案评审与验证计划)和[当前进度](../docs/status/current.md)。
