# cargoc

原生 Rust CLI：为 Cargo 提供集中管理的中间构建缓存及磁盘压力清理。最终产物仍在 Cargo 原本的目标目录：默认为工作区的 `target/`，也可由现有 Cargo 配置、环境变量或 `--target-dir` 指定。

[English](README.md) · [GitHub](https://github.com/bigfish1913/cargoc) · [crates.io](https://crates.io/crates/cargoc)

## 环境要求与安装

- 编译 cargoc 需要 **Rust 1.88+**（MSRV）。
- 受包装的 Cargo 执行受管理构建需要 **1.91+**，以支持稳定的 `build.build-dir`。较旧的 Cargo 可能可以编译 cargoc，但不能执行受管理构建。
- 支持 Windows、Linux、macOS。这是 Rust 程序，不是 Python 脚本。

通过 [crates.io](https://crates.io/crates/cargoc) 安装已发布版本：

```sh
cargo install cargoc --locked
cargoc init
cargoc build --release
```

如需最新源码版本，克隆仓库后本地安装：

```sh
git clone https://github.com/bigfish1913/cargoc.git
cd cargoc
cargo install --path . --locked
```

`--locked` 使用已提交的 `Cargo.lock`，固定依赖版本。

> **版本说明：** **0.1.1** 源码／GitHub 发行线加入 Cargo 同目录配置（`~/.cargo/cargoc.toml`）、旧配置迁移、双版本输出、Windows shim 改进和自更新。crates.io 是独立发布渠道，其 **0.1.0** 仍使用迁移说明中列出的旧配置位置。需要这些新功能时，请使用已完成的 GitHub Release 或从源码安装。

如需专用缓存根目录，用下面的命令**替代**上面的默认 `init`：

```sh
cargoc init --cache-dir D:/BuildCache/cargoc
```

`init` 不覆盖已有配置；修改请用 `cargoc config`。不要指定已有非空缓存，例如 `D:/BuildCache/cargo`：程序拒绝接管非空且没有所有权标记的目录。

## 一次引导配置与默认 Cargo 接管

配置文件默认位于 `$CARGO_HOME/cargoc.toml`，未设置 `CARGO_HOME` 时为 `~/.cargo/cargoc.toml`。与 Cargo 自己的 `config.toml` 同目录，但使用独立文件，不修改 Cargo 配置。也可以通过 `CARGOC_CONFIG` 或全局参数 `--cargoc-config PATH` 指定。

推荐首次使用直接运行：

```powershell
cargoc setup
```

它会创建配置和专用缓存目录，然后询问是否让 `cargoc` 接管默认的 `cargo` 命令。接管不会覆盖 Rustup 的真实 `cargo.exe`，而是在用户目录创建 shim，并把真实 Cargo 路径保存到配置中。

不需要交互时：

```powershell
cargoc setup --enable --cache-dir D:/BuildCache/cargoc --max-cache-gib 100 --min-free-gib 20
```

启用、检查状态或恢复原生 Cargo：

```powershell
cargoc setup --enable
cargoc setup --status
cargoc setup --disable
```

上例将预算设为 **100 GiB**，使用默认 `low_water_ratio = 0.8` 时，压力清理目标为 **80 GiB**。程序内置的默认预算仍为 **80 GiB**，不会自动改成 100 GiB。

Windows 启用后需要重新打开终端，让用户级 PATH 生效。`--disable` 只移除 cargoc 自己的 shim 和 PATH 项，不删除、不改名 Rustup 的 Cargo。Linux/macOS 不会自动修改 shell 启动文件，启用后按命令输出把 `~/.cargoc/bin` 放到 PATH 前面。

如果不想接管默认命令，也可以只初始化配置：在引导问题中回答 `N`，以后使用 `cargoc build`；原生 `cargo` 将保持不变。

### 确认实际使用的 Cargo

启用接管并刷新 PATH 后，`cargo --version` 同时展示两个版本，例如：

```text
cargo 1.99.1 (...)
cargoc 0.1.1
```

`cargo -V`、`cargo -vV`、`cargo version`、`cargo +stable --version` 同样展示两个版本。Cargo 的详细版本元数据和失败退出码保持不变。`cargoc --version` 仍只显示 cargoc 自身版本。

接管时优先使用显式指定的 `--cargo PATH`，其次沿用已有配置中的 Cargo；否则按 PATH 目录顺序查找，并排除 cargoc 自己的 shim。修改集成前会验证 Cargo 身份，保留 Rustup 的 `cargo` 调用路径，不把符号链接解析成 `rustup` 后直接启动。不覆盖或删除不属于 cargoc 的 shim 文件。

`cargoc setup --status` 会区分是否已登记、shim 是否完整安装，以及当前进程的 PATH 是否真正选中了 shim。Shell 别名、函数和可执行文件绝对路径仍可绕过接管；需要时用 PowerShell 的 `Get-Command cargo` 或 POSIX shell 的 `command -v cargo` 确认。Windows 系统级 PATH 项可能优先于用户 PATH，因此仅安装 shim 不代表所有终端都已接管。

## 日常使用

```sh
cargoc build
cargoc check --workspace
cargoc test --locked
cargoc run --release -- application-argument
cargoc clippy --all-targets -- -D warnings
cargoc status
cargoc status --json
cargoc clean --dry-run
cargoc clean
cargoc self-update --check
cargoc self-update
cargoc self-update --rollback
```

`build`、`check`、`test`、`run` 等非保留命令将参数转发给 Cargo。支持 `--manifest-path`。显式选择 Rustup 工具链可用 `cargoc exec +stable build`。

| cargoc 命令 | 行为 |
| --- | --- |
| `setup [设置项] [--enable\|--disable\|--status]` | 配置 cargoc，并可选择启用、恢复或检查默认 Cargo 接管。 |
| `init [设置项]` | 创建用户配置和带所有权标记的缓存根目录。 |
| `config [设置项]` | 显示配置，或更新指定设置项。 |
| `status [--json]` | 显示受管理缓存大小、工作区记录及缓存所在卷的剩余空间；JSON 还包含配置。 |
| `clean [--dry-run]` | 清空**所有空闲的受管理中间缓存**，不只处理过期或超预算缓存。预演报告拟删除内容，不删除缓存文件。 |
| `self-update [--check\|--rollback\|--force]` | 检查或安装最新 GitHub Release，也可恢复本地最新回滚备份。 |
| `exec <Cargo 参数...>` | 转发与 cargoc 命令重名的 Cargo 命令，或以 Cargo 选项、工具链选择器开头的调用。 |

**`cargoc clean` 不等于 `cargo clean`。** 前者不删除最终产物。相反：

```sh
cargoc exec clean
cargoc exec init new-project
```

`cargoc exec clean` 执行 **Cargo 原生 clean**，可能删除 Cargo 所选目标目录中的最终产物及选定的中间缓存，请谨慎使用。`exec` 消除命令名冲突，不是通用的“关闭管理”开关。

部分命令（例如 Cargo `fmt`、`new`、`init`、`install`、`metadata`、`fetch`、`update`）直接透传，并提示不使用受管理缓存，但仍需要 cargoc 配置。受管理命令拒绝 `--build-dir`、通过 Cargo `--config` 覆盖 `build.build-dir`，以及 Cargo `-C`；请切换工作目录或使用 `--manifest-path`。

## 更新 cargoc

`cargoc self-update` 会访问官方 GitHub Releases API，根据当前系统和 CPU 选择对应压缩包，通过 HTTPS 下载，校验 Release 提供的 SHA-256 文件，只提取预期位置的二进制、不展开其余路径，并在替换前验证版本：

```sh
cargoc self-update --check  # 只检查，不修改文件
cargoc self-update          # 下载并替换 cargoc
cargoc self-update --rollback
```

更新只替换当前运行的 `cargoc` 可执行文件，绝不修改 Rustup 的 `cargo`、Cargo 的 `config.toml`、`~/.cargo/cargoc.toml`、接管 shim 或受管理缓存，也不要求先配置缓存。替换前在 `~/.cargoc/rollback/` 按安装路径隔离保存回滚副本；`--rollback` 校验副本的 SHA-256 和版本后恢复，损坏的副本会被拒绝。同一安装的并发更新会被拒绝。只有明确需要重装相同或更旧版本时才使用 `--force`。下载上限为 128 MiB、提取的二进制上限为 64 MiB，每个 HTTP 请求限时 60 秒；不支持的平台或 ABI 直接拒绝（例如不能用 GNU 发行包替换 musl 构建）。

仅选择已公开的稳定版 Release。SHA-256 用来检测下载损坏，**不等于独立的发布者签名**；更新器信任本项目的 GitHub 账号、Release 资产及 HTTPS。必须先由发布工作流成功发布兼容的 Release，才有可下载的更新。

默认更新是显式触发的：普通 Cargo／构建调用不会主动连接 GitHub。**自动更新通过操作系统的计划任务主动启用**，不是隐式后台服务；安装 cargoc 不会替你创建任务。确认发布策略后，可用下面任一方式每日自动检查并安装；已是最新版本时不改动可执行文件。

Windows PowerShell（为当前用户创建任务，无需管理员权限；执行时该用户需已登录）：

```powershell
$action = New-ScheduledTaskAction -Execute (Get-Command cargoc -CommandType Application).Source -Argument 'self-update'
$trigger = New-ScheduledTaskTrigger -Daily -At '10:00'
Register-ScheduledTask -TaskName 'cargoc-self-update' -Action $action -Trigger $trigger -Description 'Update cargoc from its stable GitHub Release'
# 关闭自动更新：
# Unregister-ScheduledTask -TaskName 'cargoc-self-update' -Confirm:$false
```

Linux/macOS：用 `crontab -e` 添加下面一行（若安装在其他位置，请调整可执行文件路径）：

```cron
0 10 * * * "$HOME/.cargo/bin/cargoc" self-update >> "$HOME/.cargoc-self-update.log" 2>&1
```

删除该行即可关闭自动更新。不要仅为更新用户目录中的程序而使用管理员／root 权限；更新由其他包管理器托管的安装前，请保留可恢复副本。

## 配置

cargoc 只写入**自己的** TOML 配置，不修改全局 Cargo 配置。`--cargoc-config PATH` 优先于环境变量 `CARGOC_CONFIG`；否则使用默认路径：

- 所有平台：设置 `CARGO_HOME` 时使用 `$CARGO_HOME/cargoc.toml`。
- Windows 默认：`%USERPROFILE%/.cargo/cargoc.toml`。
- Linux/macOS 默认：`~/.cargo/cargoc.toml`。

Cargo 自己的 `config.toml` 保持独立，绝不修改。新默认文件不存在时，会从旧位置 `%APPDATA%/cargoc/config.toml`（Windows）或 `$XDG_CONFIG_HOME/cargoc/config.toml` / `~/.config/cargoc/config.toml`（Linux/macOS）复制经过校验的配置，保留原文件作为恢复副本，不覆盖已有的新配置。显式指定 `--cargoc-config` / `CARGOC_CONFIG` 时不迁移。此前已启用默认 Cargo 接管的用户，运行一次 `cargoc setup --enable`，即可刷新入口固定的配置路径；缓存目录、阈值不会改变。

```sh
cargoc --cargoc-config ./cargoc-local.toml init --cache-dir D:/BuildCache/cargoc
cargoc --cargoc-config ./cargoc-local.toml build
cargoc config
cargoc config --max-cache-gib 80 --low-water-ratio 0.8
cargoc config --auto-clean false
cargoc config --auto-clean true --abort-free-gib 1
```

配置文件必须放在可清理缓存根目录之外。以下设置同时适用于 `init`、`config`；TOML 键名用下划线而不是连字符。

| 参数 | 默认值 | 含义 |
| --- | --- | --- |
| `--cache-dir PATH` | 平台缓存目录下的 `cargoc/cache` | 专用受管理根目录；命令行路径转换为绝对路径。 |
| `--max-cache-gib N` | `80` | 受管理中间缓存的软预算，不是全部磁盘用量；必须为正数。 |
| `--low-water-ratio N` | `0.8` | 压力清理目标比例，严格介于 0 和 1；默认目标 **64 GiB**。 |
| `--min-free-gib N` | `20` | 缓存卷的期望剩余空间；启动受管理 Cargo 调用前也会检查。 |
| `--abort-free-gib N` | `1` | 紧急停止阈值，只停止本次 Cargo 进程树；`0` 禁用此停止功能。 |
| `--max-age-days N` | `7` | 按最后一次 cargoc 使用时间判断过期，**不是文件 mtime**；`0` 禁用按年龄清理。 |
| `--poll-seconds N` | `5` | 自动清理间隔，范围 `0.1`～`86400` 秒；紧急空间检查更频繁（每轮最多一次，默认约 250 毫秒）。 |
| `--auto-clean true\|false` | `true` | 在受管理命令开始前、运行中、结束后自动清理；关闭后仍保留启动空间检查和紧急停止。 |
| `--cargo PATH` | `cargo` | 真实 Cargo 可执行文件，不是 shell 命令；不能指回 cargoc。 |

默认缓存基目录：Windows 为 `%LOCALAPPDATA%`，Linux/macOS 为 `$XDG_CACHE_HOME`；未设置时回退到 `~/.cache`。数值必须有限且非负，并满足上表更严格的限制。`min_free_gib` 非零时，`abort_free_gib` 不得大于它。

## 所有权与清理安全边界

- 按解析后的工作区绝对路径计算 SHA256（Windows 上统一大小写和扩展路径前缀），缓存位于 `<cache-dir>/workspaces/<SHA256>`。根标记 `.cargoc-root.json` 和校验过的工作区记录识别受管理数据。
- 自动清理按最后使用时间从旧到新选择空闲缓存：过期、超过受管理预算或空间不足时触发。压力清理尽力达到低水位目标和期望剩余空间，但受安全候选项限制。
- 工作区租约锁、清理器锁和 Cargo 原生锁保护活跃缓存。同一工作区的并发 cargoc 构建会被拒绝，不共享可写缓存；已锁定候选项会跳过。
- 手动 `clean` 即使在 `auto_clean = false` 时也处理所有空闲受管理缓存，但不清理未知内容。清理后可能保留部分锁文件和目录。
- 切换 `cache_dir` **不会**迁移或删除旧根目录。既有非 cargoc 缓存保持不变，也不计入预算。不要手动添加所有权标记接管旧缓存。
- 包装器仅为每次受管理调用提供 `build.build-dir`，不修改 Cargo 原有 target-dir 配置，也不移动现有产物。旧缓存应由其所有者另行处理。
- 紧急停止仅针对本次调用拥有的 Windows Job Object 或 Unix 进程组，不按进程名终止无关 Cargo/rustc。这是进程所有权边界，不是任意构建脚本的沙箱。

**这是软缓存预算，不是硬磁盘配额，也不保证磁盘绝不会写满。** 当前构建可能超预算，活跃缓存受保护，空间检查是周期性的。最终产物、注册表、其他缓存和无关程序用量不在预算内。缓存与最终产物位于不同卷时，检查缓存卷不能保护产物卷。启用的安全清理结束后，如果空间仍低于配置的最小/紧急阈值，将拒绝启动受管理构建。

只有实际通过 cargoc 启动的命令才受管理。仅安装 cargoc 不会改变普通 `cargo` 命令；启用 shim 并使其排在 PATH 前面后，直接执行 `cargo build`、`cargo test` 等命令就会经过 cargoc。IDE 或脚本若使用真实 Cargo 可执行文件的绝对路径，仍会绕过接管。

自动清理和 Cargo 退出事件写入 cargoc 配置旁的 `events.jsonl`，日志达到约 2 MiB 时轮换。清理或日志写入失败会警告，但不会覆盖 Cargo 的退出码，也不会强删不安全的候选项。空间统计使用逻辑字节数；硬链接等情况会使实际释放空间与逻辑删除量不同。

专用缓存根目录属于可丢弃存储，**不要在里面存源码、手写笔记或最终 `target-dir` 产物**。缓存路径中的符号链接和重解析点会被拒绝。本工具面向可信用户的日常维护，不提供抵御其他进程恶意替换文件系统条目的安全沙箱。

## 预编译发布

推送到 `main` 后，`.github/workflows/release.yml` 自动发布新的项目版本，使用 GitHub 托管的原生 runner 构建以下平台：

| 平台 | 压缩包 |
| --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc`（`.zip`） |
| Windows ARM64 | `aarch64-pc-windows-msvc`（`.zip`） |
| Linux x64 | `x86_64-unknown-linux-gnu`（`.tar.gz`） |
| Linux ARM64 | `aarch64-unknown-linux-gnu`（`.tar.gz`） |
| macOS Intel | `x86_64-apple-darwin`（`.tar.gz`） |
| macOS Apple Silicon | `aarch64-apple-darwin`（`.tar.gz`） |

发行构建成功后，可在 [GitHub Releases](https://github.com/bigfish1913/cargoc/releases) 下载，例如 `cargoc-v0.1.1-x86_64-pc-windows-msvc.zip`。每个包包含 `cargoc` 可执行文件、双语 README 和 MIT 许可证，另附 `.sha256` 校验文件；Release 还提供汇总的 `SHA256SUMS`。

选择对应架构的压缩包，核对 SHA-256 后解压，将 **`cargoc` / `cargoc.exe`** 放入 PATH 中已有的目录即可，不要覆盖 Rustup 的 `cargo`。安装预编译程序不需要 Rust 编译器，但受管理构建仍需要 Cargo 1.91+。Linux 包使用 glibc，在 Ubuntu 22.04（glibc 2.35）上构建，不提供 Alpine/musl 版本。Windows/macOS 程序未进行 Authenticode/Developer ID 签名或公证。

维护者发布时，先将 `Cargo.toml` **及 `Cargo.lock`** 更新到新版本（例如 `0.1.2`），再把审核后的改动提交并推送到 `main`：

```sh
git add Cargo.toml Cargo.lock # 同时暂存审核后的源码／文档改动
git commit -m "Release 0.1.2"
git push origin main
```

不需要手动打标签。格式、Clippy、MSRV、六个平台的原生测试／构建、打包后二进制测试及校验和验证全部通过后，工作流自动在**实际测试的提交**上创建 `v{version}`，将资源上传至草稿，再公开发布。它不会自行递增项目版本，也不会回写 `main`。推送的版本已经发布时，跳过重复发布；下一次发行需要提升版本号。仍支持手动推送 `v*` 标签，但必须与项目版本完全一致。

`0.1.2-rc.1` 等版本会标记为预发布。不覆盖已有 Release、资源或指向其他提交的标签。上传失败可能留下草稿，重试前需检查并删除该草稿。由 `GITHUB_TOKEN` 创建的标签／Release 不会再次触发发行任务。此流程不向 crates.io 发布。

只想预编译验证时，在 **Actions → Release → Run workflow** 手动运行（`workflow_dispatch`），产物作为 Actions Artifacts 保留 14 天，不创建标签或公开 Release。GitHub 托管 runner 标签可能调整，届时需更新矩阵。

## 开发

CI 使用稳定版 Rust 及 `rustfmt`、`clippy`，在 Windows、Ubuntu、macOS 上执行：

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

工作流通过 `CARGO_TARGET_DIR` 和 `CARGO_BUILD_BUILD_DIR` 将最终产物、中间产物分别放入各任务本地临时目录，直接调用 Cargo 而非 cargoc，不修改全局配置。另有 Rust 1.88 任务执行全部目标的检查，验证声明的 MSRV。

## 贡献者

欢迎提交代码、问题反馈和文档改进。完整名单见 GitHub [贡献者列表](https://github.com/bigfish1913/cargoc/graphs/contributors)。

[![cargoc 贡献者](https://contrib.rocks/image?repo=bigfish1913/cargoc)](https://github.com/bigfish1913/cargoc/graphs/contributors)

贡献者头像由 [contrib.rocks](https://contrib.rocks) 动态生成。

## Star 趋势

[![cargoc Star 趋势图](https://api.star-history.com/svg?repos=bigfish1913/cargoc&type=Date)](https://www.star-history.com/#bigfish1913/cargoc&Date)

[查看交互式 Star 趋势](https://www.star-history.com/#bigfish1913/cargoc&Date) · [前往仓库 Star 列表](https://github.com/bigfish1913/cargoc/stargazers)

趋势图和头像依赖第三方服务，可能因 API 限流暂时不可用。图片无法加载时，仍可使用上面的文字链接。

## 许可证

[MIT](LICENSE)。Copyright 2026 cargoc contributors.
