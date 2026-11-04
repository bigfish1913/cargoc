# cargoc

原生 Rust CLI：为 Cargo 提供集中管理的中间构建缓存及磁盘压力清理。最终产物仍在 Cargo 原本的目标目录：默认为工作区的 `target/`，也可由现有 Cargo 配置、环境变量或 `--target-dir` 指定。

[English](README.md)

## 环境要求与安装

- 编译 cargoc 需要 **Rust 1.88+**（MSRV）。
- 受包装的 Cargo 执行受管理构建需要 **1.91+**，以支持稳定的 `build.build-dir`。较旧的 Cargo 可能可以编译 cargoc，但不能执行受管理构建。
- 支持 Windows、Linux、macOS。这是 Rust 程序，不是 Python 脚本。

在源码目录中执行：

```sh
cargo install --path . --locked
cargoc init
cargoc build --release
```

`--locked` 需要源码仓库中已提交的 `Cargo.lock`。此处不假设项目已发布到 crates.io，也不提供未经确认的仓库或发行包地址。

如需专用缓存根目录，用下面的命令**替代**上面的默认 `init`：

```sh
cargoc init --cache-dir D:/BuildCache/cargoc
```

`init` 不覆盖已有配置；修改请用 `cargoc config`。不要指定已有非空缓存，例如 `D:/BuildCache/cargo`：程序拒绝接管非空且没有所有权标记的目录。

## 一次引导配置与默认 Cargo 接管

配置文件默认位于 Windows 的 `%APPDATA%\cargoc\config.toml`，Linux/macOS 位于 `$XDG_CONFIG_HOME/cargoc/config.toml`（未设置时为 `~/.config/cargoc/config.toml`）。也可以通过 `CARGOC_CONFIG` 或全局参数 `--cargoc-config PATH` 指定。

推荐首次使用直接运行：

```powershell
cargoc setup
```

它会创建配置和专用缓存目录，然后询问是否让 `cargoc` 接管默认的 `cargo` 命令。接管不会覆盖 Rustup 的真实 `cargo.exe`，而是在用户目录创建 shim，并把真实 Cargo 路径保存到配置中。

不需要交互时：

```powershell
cargoc setup --enable --cache-dir D:/BuildCache/cargoc --max-cache-gib 80 --min-free-gib 20
```

启用和恢复：

```powershell
cargoc setup --status
cargoc setup --disable
```

Windows 启用后需要重新打开终端，让用户级 PATH 生效。`--disable` 只移除 cargoc 自己的 shim 和 PATH 项，不删除、不改名 Rustup 的 Cargo。Linux/macOS 不会自动修改 shell 启动文件，启用后按命令输出把 `~/.cargoc/bin` 放到 PATH 前面。

如果不想接管默认命令，也可以只初始化配置：在引导问题中回答 `N`，以后使用 `cargoc build`；原生 `cargo` 将保持不变。

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
```

`build`、`check`、`test`、`run` 等非保留命令将参数转发给 Cargo。支持 `--manifest-path`。显式选择 Rustup 工具链可用 `cargoc exec +stable build`。

| cargoc 命令 | 行为 |
| --- | --- |
| `init [设置项]` | 创建用户配置和带所有权标记的缓存根目录。 |
| `config [设置项]` | 显示配置，或更新指定设置项。 |
| `status [--json]` | 显示受管理缓存大小、工作区记录及缓存所在卷的剩余空间；JSON 还包含配置。 |
| `clean [--dry-run]` | 清空**所有空闲的受管理中间缓存**，不只处理过期或超预算缓存。预演报告拟删除内容，不删除缓存文件。 |
| `exec <Cargo 参数...>` | 转发与 cargoc 命令重名的 Cargo 命令，或以 Cargo 选项、工具链选择器开头的调用。 |

**`cargoc clean` 不等于 `cargo clean`。** 前者不删除最终产物。相反：

```sh
cargoc exec clean
cargoc exec init new-project
```

`cargoc exec clean` 执行 **Cargo 原生 clean**，可能删除 Cargo 所选目标目录中的最终产物及选定的中间缓存，请谨慎使用。`exec` 消除命令名冲突，不是通用的“关闭管理”开关。

部分命令（例如 Cargo `fmt`、`new`、`init`、`install`、`metadata`、`fetch`、`update`）直接透传，并提示不使用受管理缓存，但仍需要 cargoc 配置。受管理命令拒绝 `--build-dir`、通过 Cargo `--config` 覆盖 `build.build-dir`，以及 Cargo `-C`；请切换工作目录或使用 `--manifest-path`。

## 配置

cargoc 只写入**自己的** TOML 配置，不修改全局 Cargo 配置。`--cargoc-config PATH` 优先于环境变量 `CARGOC_CONFIG`；否则使用默认路径：

- Windows：`%APPDATA%/cargoc/config.toml`。
- Linux/macOS：`$XDG_CONFIG_HOME/cargoc/config.toml`，未设置时为 `~/.config/cargoc/config.toml`。
- Windows 未设置 `%APPDATA%` 时同样回退到 `~/.config/cargoc/config.toml`。

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

只有实际通过 cargoc 启动的命令才受管理。直接执行 `cargo`，或 IDE 使用 Cargo 的绝对路径，都会绕过它。请显式配置集成；安装 cargoc 不会透明替换 Cargo。

自动清理和 Cargo 退出事件写入 cargoc 配置旁的 `events.jsonl`，日志达到约 2 MiB 时轮换。清理或日志写入失败会警告，但不会覆盖 Cargo 的退出码，也不会强删不安全的候选项。空间统计使用逻辑字节数；硬链接等情况会使实际释放空间与逻辑删除量不同。

专用缓存根目录属于可丢弃存储，**不要在里面存源码、手写笔记或最终 `target-dir` 产物**。缓存路径中的符号链接和重解析点会被拒绝。本工具面向可信用户的日常维护，不提供抵御其他进程恶意替换文件系统条目的安全沙箱。

## 开发

CI 使用稳定版 Rust 及 `rustfmt`、`clippy`，在 Windows、Ubuntu、macOS 上执行：

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

工作流通过 `CARGO_TARGET_DIR` 和 `CARGO_BUILD_BUILD_DIR` 将最终产物、中间产物分别放入各任务本地临时目录，直接调用 Cargo 而非 cargoc，不修改全局配置。另有 Rust 1.88 任务执行全部目标的检查，验证声明的 MSRV。

## 许可证

[MIT](LICENSE)。Copyright 2026 cargoc contributors.
