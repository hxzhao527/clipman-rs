# Packaging

MVP 使用系统 GTK4 动态链接，不制作 AppImage 或 Flatpak。

## 用户级安装

```bash
./scripts/install-user.sh
```

脚本执行：

- `cargo build --release`；
- 安装二进制到 `$XDG_BIN_HOME` 或 `~/.local/bin`；
- 安装 desktop 文件到 `$XDG_DATA_HOME/applications` 或 `~/.local/share/applications`；
- 安装 SVG 图标到 hicolor scalable apps；
- 可用时刷新 desktop 数据库和图标缓存。

安装后请确认 `~/.local/bin` 在 `PATH` 中。

## 应用 ID

稳定 application ID：

```text
io.github.clipman_rs.Clipman
```

该 ID 同时用于 GtkApplication 单实例和 desktop 文件身份。

## 自动启动

程序不管理 autostart。需要登录自动启动时，请由 labwc 或桌面会话配置启动
`clipman-rs`。
