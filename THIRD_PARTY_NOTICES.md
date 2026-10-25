# 第三方来源与许可

本项目 LICENSE 只约束 RadishLedger 原创材料；第三方代码、模板与资产保留其原有许可。精确依赖图由 [Cargo.lock](Cargo.lock) 与 [pubspec.lock](apps/m0_probe/pubspec.lock) 固定，本文件登记接入实验的直接来源和必要归属。

| 材料 | 版本 / 来源 | 许可与使用 |
| --- | --- | --- |
| Flutter SDK、Android / iOS 模板 | `3.44.0`，revision `559ffa3f75e7402d65a8def9c28389a9b2e6fe42`；[Flutter](https://github.com/flutter/flutter) | BSD-3-Clause；保留[模板许可全文](third-party/flutter-template-license.txt) |
| 默认 App 图标 / launch PNG | SDK 与 `flutter_template_images 5.0.0`；[模板图像包](https://pub.dev/packages/flutter_template_images/versions/5.0.0) | BSD-3-Clause；保留[图像包许可](third-party/flutter-template-images-license.txt)，全部 PNG 匹配上述正式来源，不来自私人参考 |
| Rust runtime / 标准库 | `1.96.0`；[Rust](https://github.com/rust-lang/rust) | MIT OR Apache-2.0 及标准库所含材料；保留并打包[官方标准库许可集](apps/m0_probe/assets/rust-library-notices.html)，编译器本身不随 App 分发 |
| rusqlite / libsqlite3-sys | `0.41.0` / `0.39.1`；[rusqlite](https://github.com/rusqlite/rusqlite) | MIT；用于加密 SQLite 绑定 |
| sqlcipher-src | `419.0.6+sqlcipher-4.19.0-sqlite-3.53.4-libtomcrypt-1.18.2`；[源码封装](https://github.com/LucaCappelletti94/sqlcipher-src) | 包声明 MIT AND BSD-3-Clause AND blessing AND WTFPL；包装代码 MIT，Copyright 2026 Luca Cappelletti；内含材料分别保留原许可 |
| SQLCipher / SQLite | `4.19.0 community` / `3.53.4`，由固定 sqlcipher-src 引入；[SQLCipher](https://github.com/sqlcipher/sqlcipher) | SQLCipher BSD-3-Clause，SQLite public domain；收集内含 LICENSE-sqlcipher 和 sqlite3.h 首段声明 |
| libtomcrypt 源码材料 | `1.18.2`，内含于 sqlcipher-src；[LibTomCrypt](https://github.com/libtom/libtomcrypt) | public domain / WTFPL 双许可全文保留在保守来源清单；当前 Android / Apple native 构建未选择或编译该提供方 |
| OpenSSL | `4.0.3`，本地 openssl-src `400.0.2+4.0.3`；[完整来源 / 最小差异](third-party/openssl-src-patch.md)；[OpenSSL](https://github.com/openssl/openssl) | Apache-2.0；Android / Linux 的静态密码底层，Apple 使用系统 CommonCrypto |
| OpenSSL ARM 能力补丁 | [补丁](third-party/openssl-armcap-sve.patch)、[来源与哈希](third-party/openssl-armcap-sve.json)；基于 OpenSSL 4.0.3 的 crypto/armcap.c | Apache-2.0，Copyright 2011–2026 The OpenSSL Project Authors；许可全文在 native notices 的 Apache-2.0 组。已应用到完整上游源码副本，仅增加 SVE 前置条件；能力诊断程序为项目原创 |
| openssl-sys 最小源码补丁 | `0.9.117`；[来源 / 差异](third-party/openssl-sys-patch.md)、[原包哈希清单](third-party/openssl-sys-upstream.json) | MIT；完整保留上游 81 文件，仅改 openssl-src 约束及 JSON 末尾 LF；未改绑定实现 |
| 密码验证公开向量 | [NIST SP800-38A F.2.5](https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-38a.pdf)、[RFC4231 §4.2](https://www.rfc-editor.org/rfc/rfc4231.html)、[RFC7914 §11](https://www.rfc-editor.org/rfc/rfc7914.html) | NIST 美国政府出版物；RFC 分别 © ISOC 2005 / IETF Trust 2016，按各文档版权声明。仅使用公开输入 / 期望数值，未移植算法代码或说明正文；验证程序为项目原创，不随 App 打包 |
| flutter_rust_bridge / generator | `2.13.0`；[FRB](https://github.com/fzyzcjy/flutter_rust_bridge) | MIT，Copyright 2021 fzyzcjy；Rust 与 Dart runtime 共用上游同版许可，生成代码保留标记 |
| Tokio（FRB 传递依赖） | `1.53.3`；[Tokio](https://github.com/tokio-rs/tokio)、[crate](https://crates.io/crates/tokio/1.53.3) | MIT；异步运行时，按 RUSTSEC-2026-0386 升级；许可正文未变，全文由 native notices 保留 |
| serde / serde_json | `1.0.228` / `1.0.150`；[serde](https://github.com/serde-rs/serde)、[serde_json](https://github.com/serde-rs/json) | MIT OR Apache-2.0；结构化输入 / 输出，不形成第二份金额真相 |
| getrandom / zeroize / tempfile | `0.3.4` / `1.8.2` / `3.27.0`；[getrandom](https://github.com/rust-random/getrandom)、[zeroize](https://github.com/RustCrypto/utils)、[tempfile](https://github.com/Stebalien/tempfile) | MIT OR Apache-2.0；合成身份、内存钥和隔离临时目录 |
| chacha20poly1305 | `0.11.0`；[RustCrypto AEAD](https://github.com/RustCrypto/AEADs) | MIT OR Apache-2.0；XChaCha20-Poly1305 认证封装，仅实验格式 |
| argon2 | `0.6.0`；[RustCrypto password-hashes](https://github.com/RustCrypto/password-hashes) | MIT OR Apache-2.0；Argon2id v19，固定有界 profile |
| hkdf / sha2 | `0.13.0` / `0.11.1`；[RustCrypto KDFs](https://github.com/RustCrypto/KDFs)、[RustCrypto hashes](https://github.com/RustCrypto/hashes) | MIT OR Apache-2.0；用途隔离与非秘密完整性核对 |
| ctutils（密码传递依赖） | `0.4.4`；[RustCrypto utils](https://github.com/RustCrypto/utils)、[crate](https://crates.io/crates/ctutils/0.4.4) | Apache-2.0 OR MIT；恒定时间工具，按 RUSTSEC-2026-0385 升级；许可全文由 native notices 保留 |
| libc | `0.2.189`；[libc](https://github.com/rust-lang/libc) | MIT OR Apache-2.0；实际 code asset 定位与上游 bridge 兼容 |
| ffi / hooks / code_assets | `2.2.0` / `1.0.2` / `1.0.0`；[dart-lang/native](https://github.com/dart-lang/native) | BSD-3-Clause；Dart native 调用和构建资产 |
| Material Icons 与 Flutter / Dart 传递包 | SDK 内含字体与 `pubspec.lock` 固定包；[Material Icons](https://github.com/google/material-design-icons) | 字体 Apache-2.0，各包许可由 SDK / pub 包随附并进入 Flutter LICENSE asset；没有额外引入第三方字体或图标资产 |
| Temurin JDK | `17.0.20.1+1`；[Adoptium](https://github.com/adoptium/temurin17-binaries) | GPL-2.0 with Classpath Exception；仅本地构建工具，未提交或随 App 分发 |
| Gradle / AGP / cargo-expand | `9.1.0` / `9.0.1` / `1.0.118`；各官方发行与 registry 包 | 构建 / 生成工具，仅在缓存中使用，未作为 App 资产引入 |

Rust 传递包、SQLCipher 与 OpenSSL 的许可全文由 [collect-native-notices.py](scripts/collect-native-notices.py) 从锁定缓存收集到 [native-notices.txt](apps/m0_probe/assets/native-notices.txt)，并作为 Flutter asset 随实验包保留。本地 openssl-sys / openssl-src 虽无 registry source，仍显式记录补丁来源并收集完整许可。openssl-src 内含的 Text::Template 1.56 构建工具按其 GPL-1.0-or-later OR Artistic-1.0 许可原文保留并进入 notices，不作为运行时密码提供方。该清单含两端 arm64 与验证宿主的 build dependencies，是保守并集；不能当作最终二进制扫描。完全相同的许可正文分组保留各包名、版本与来源。allo-isolate 依包内 Apache-2.0 LICENSE 登记；dart-sys 采用声明的 Apache-2.0 选项，额外保留其内含 Dart SDK 的 BSD notice。sqlcipher-src 的根 MIT 不代替内含 SQLCipher / SQLite / libtomcrypt 材料；生成器显式收集嵌套许可和 SQLite 首段声明，缺失或为空即失败。

生成器需要本机已取得锁定包、Flutter pub 配置和固定 Rust 工具链，不访问网络或安装工具；依赖变化后重新生成并运行 `--check`。只规范许可的文本行尾，不删除原归属。Flutter 的自动 LICENSE asset 与本项目 native notices 各覆盖对应来源，公开发行前复核实际打包内容及新增资产。

兄弟项目仅提供抽象工程经验，没有导入其实现包。私人金融 / UI 参考的原图及任何衍生材料不在上述资产中，不进入仓库、Git、公开展示或外部服务。
