# clipman-rs

labwc/Wayland 下面向菜单位置的文本剪贴板历史管理器。

当前已完成持久化、配置和历史窗口主要功能，已经实现：

- `gtk4` 历史窗口；
- `wl-clipboard-rs` data-control 后台监听；
- 普通剪贴板和 Primary Selection 捕获；
- 多 `text/*` MIME 捕获与恢复；
- `ksni` StatusNotifierItem 托盘和动态菜单；
- 4 MiB 记录限制、空白判定、fingerprint 去重；
- 菜单空白移除和 15 grapheme 预览；
- 自身写入事件抑制；
- `GtkApplication` 单实例；
- SQLx/SQLite migration 和事务化持久化；
- 普通历史、Primary、顺序和设置的重启恢复；
- 配置窗口、Primary 开关和两个数量配置；
- 搜索、单条删除和清空确认；
- 中英文 i18n；
- MIME 读取超时和异常源保护。

真实浏览器、XWayland 和完整人工验收仍在后续阶段。

## 构建与运行

需要 GTK4 开发包和 Rust 工具链：

```bash
cargo build
cargo run
```

程序启动后驻留系统托盘。Waybar 需要启用通用 `tray` 模块。

数据库默认保存到：

```text
$XDG_DATA_HOME/clipman-rs/clipman.sqlite3
```

未设置 `$XDG_DATA_HOME` 时使用 `~/.local/share/clipman-rs/clipman.sqlite3`。

## 用户级安装

```bash
./scripts/install-user.sh
```

安装脚本会构建 release 版本，并安装二进制、desktop 文件和 SVG 图标到用户目录。
详细说明见 [packaging/README.md](packaging/README.md)。

## 多 MIME 验证

启动应用后，可以运行辅助源：

```bash
cargo run --example multi_mime_source
```

它会提供 `text/plain`、`text/plain;charset=utf-8`、`text/html` 和 `text/rtf` 四种表示，可用于验证捕获及恢复。

## 文档

- [需求与交互规范](docs/requirements.md)
- [技术设计与架构方案](docs/design.md)
- [当前实现状态与待办](docs/status.md)
- [已知问题](docs/known-issues.md)
- [打包与安装](packaging/README.md)
