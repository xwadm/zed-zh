# Zed
[![Zed](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/zed-industries/zed/main/assets/badge/v0.json)](https://zed.dev)
[![CI](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml/badge.svg)](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml)

欢迎使用 Zed —— 一款由 [Atom](https://github.com/atom/atom) 与 [Tree-sitter](https://github.com/tree-sitter/tree-sitter) 开发者打造的高性能多人协作代码编辑器。

---

### 安装方法
在 macOS、Linux 和 Windows 系统上，你可以[直接下载 Zed](https://zed.dev/download)，或通过系统包管理器安装（[macOS](https://zed.dev/docs/installation#macos) / [Linux](https://zed.dev/docs/linux#installing-via-a-package-manager) / [Windows](https://zed.dev/docs/windows#package-managers)）。

其他平台暂未支持：
- 网页版（[跟踪议题](https://github.com/zed-industries/zed/issues/5396)）

### 参与 Zed 开发
- [为 macOS 构建 Zed](./docs/src/development/macos.md)
- [为 Linux 构建 Zed](./docs/src/development/linux.md)
- [为 Windows 构建 Zed](./docs/src/development/windows.md)

### 贡献代码
查看 [CONTRIBUTING.md](./CONTRIBUTING.md) 了解你可以为 Zed 做出的贡献。

此外，我们正在招聘！查看[招聘页面](https://zed.dev/jobs)了解开放岗位。

### 许可协议
第三方依赖的许可证信息必须正确配置，CI 才能通过。

我们使用 [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) 自动遵守开源许可证规范。如果 CI 运行失败，请检查以下内容：
- 若你创建的 crate 提示 `未指定许可证` 错误，请在该 crate 的 `Cargo.toml` 文件中 `[package]` 节点下添加 `publish = false`。
- 若依赖项提示 `未满足许可证要求` 错误，请先确认该项目的许可证类型，以及当前系统是否满足许可证合规要求（如有疑问请咨询律师）。确认无误后，将该许可证的 SPDX 标识符添加到 `script/licenses/zed-licenses.toml` 文件的 `accepted` 数组中。
- 若 `cargo-about` 无法找到依赖的许可证，请按照 [`cargo-about` 官方文档](https://embarkstudios.github.io/cargo-about/cli/generate/config.html#crate-configuration) 的说明，在 `script/licenses/zed-licenses.toml` 文件末尾添加澄清字段。

## 赞助支持
Zed 由 **Zed Industries, Inc.**（营利性公司）开发维护。

如果你想为该项目提供资金支持，可以通过 GitHub Sponsors 进行赞助。
赞助款项将直接用于 Zed Industries 的公司日常运营。
赞助行为不附带任何额外福利或权益。