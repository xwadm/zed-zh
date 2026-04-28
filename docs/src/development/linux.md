# 在 Linux 上构建 Zed
> 适用于 Zed 开发的 Linux 构建指南

## 克隆仓库
克隆 [Zed 代码仓库](https://github.com/zed-industries/zed)。

## 安装依赖
1. 安装 [rustup](https://www.rust-lang.org/tools/install)
2. 安装系统依赖库（自动安装）
   ```sh
   script/linux
   ```
   如需手动安装，所需包列表可查看 `script/linux` 文件。

## 从源码构建
依赖安装完成后，使用 Cargo 构建：

**调试版编辑器**
```sh
cargo run
```

**运行测试**
```sh
cargo test --workspace
```

**发行版 CLI**
```sh
cargo run -p cli
```

## 安装开发构建版
本地安装构建好的 Zed：
```sh
./script/install-linux
```
该命令会以发行模式构建 `zed` 和 `cli`，将二进制文件安装到 `~/.local/bin/zed`，并将 `.desktop` 文件安装到 `~/.local/share`。

---
### 链接器错误解决方案（常见问题）
如果出现类似以下错误：
```
undefined reference to `__isoc23_sscanf'
undefined reference to `__isoc23_strtol'
```
**原因**：`aws-lc-rs` 不支持 GCC 14 及以上版本。

**解决方法**：
```sh
export REMOTE_SERVER_TARGET=x86_64-unknown-linux-gnu; script/install-linux
```

## Wayland 与 X11
Zed 同时支持 X11 和 Wayland，运行时自动检测。
如需在 Wayland 上强制使用 X11 模式：
```sh
WAYLAND_DISPLAY='' cargo run
```

## 打包 Zed 说明（发行版维护者）
Zed 包含两个核心二进制文件：
1. 构建 `crates/cli`，命名为 `zed` 并放入 `$PATH`
2. 构建 `crates/zed`，放入路径：
   - 推荐：`$PATH/to/cli/../../libexec/zed-editor`
   - 兼容（Arch）：`$PATH/to/cli/../../lib/zed/zed-editor`
3. 桌面文件模板：`crates/zed/resources/zed.desktop.in`，使用 `envsubst` 填充变量
4. 设置环境变量 `ZED_UPDATE_EXPLANATION` 可禁用自动更新
5. 更新 `crates/zed/RELEASE_CHANNEL` 为 `nightly`/`preview`/`stable`

## Flatpak 构建
> Zed 的 Flatpak 会在启动时退出沙箱，依赖沙箱的工作流可能异常。

1. 安装 Flatpak
2. 安装依赖：`script/flatpak/deps`
3. 构建打包：`script/flatpak/bundle-flatpak`
4. 安装包位于：`target/release/{app-id}.flatpak`

## 内存分析
使用 `heaptrack` 分析内存泄漏：
```sh
sudo apt install heaptrack heaptrack-gui
cargo install cargo-heaptrack
cargo heaptrack -b zed
```

## CPU 性能分析（Perf）
Zed 高 CPU 占用时使用：
1. 获取 PID：`ps -eo size,pid,comm | grep zed`
2. 安装 perf：`sudo apt install linux-tools`
3. 记录数据：`sudo perf record -p <PID>`
4. 生成火焰图（需完整调试信息）

## 故障排查
### 依赖使用不稳定特性
```sh
cargo clean
cargo build
```
