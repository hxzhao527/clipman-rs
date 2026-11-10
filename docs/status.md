# clipman-rs 实现状态

> 更新日期：2026-10-27  
> 当前阶段：持久化、配置和历史窗口主要功能完成，进入完善阶段  
> 需求基线：[requirements.md](requirements.md)  
> 技术设计：[design.md](design.md)  
> 已知问题：[known-issues.md](known-issues.md)

## 1. 总体状态

当前代码已经可以作为原型运行，并完成 Wayland 剪贴板、Primary Selection、动态托盘和基础 GTK 窗口的技术验证。

当前版本还不是完整 MVP，主要原因是还缺少真实浏览器、XWayland 和完整人工验收。

| 范围 | 状态 |
|---|---|
| GTK4 与 Rust 工程骨架 | 已实现 |
| Wayland 后台剪贴板监听 | 已实现并实测 |
| Primary Selection 捕获 | 已实现并实测 |
| 多 `text/*` MIME 捕获和恢复 | 已实现并实测 |
| 自身写入事件抑制 | 已实现并实测 |
| ksni 系统托盘 | 已实现并实测 |
| 动态托盘菜单 | 已实现并实测 |
| 基础“全部记录”窗口 | 已实现 |
| SQLite / SQLx 持久化 | 已实现并实测 |
| 搜索和单条删除 | 已实现 |
| 配置窗口和即时保存 | 已实现 |
| 完整中英文 i18n | 已实现并实测 |
| XWayland 双向验收 | 未完成 |

## 2. 已实现并验证

### 2.1 工程与运行

- 使用 Rust 2024 edition。
- 使用 `gtk4` Rust 绑定构建窗口。
- 使用 `GtkApplication` 提供应用生命周期和单实例行为。
- 使用 Tokio 后台 runtime。
- 使用 `ksni` 提供 StatusNotifierItem。
- 使用 `tracing` 输出运行日志，不输出剪贴板明文。

相关代码：

- [Cargo.toml](../Cargo.toml)
- [main.rs](../src/main.rs)

### 2.2 Wayland 剪贴板

阶段 0 实测确认 GDK 后台客户端不能可靠监听其他应用产生的全局 selection，因此剪贴板层改用 data-control。

已实现：

- 通过 `wl-clipboard-rs` 监听普通剪贴板；
- 通过同一 watcher 监听 Primary Selection；
- 使用 `ext-data-control` 或 `wlr-data-control`；
- 跳过启动时已经存在的选区状态；
- 从 data-control offer 读取 MIME；
- 使用 `copy_multi` 恢复多个 MIME；
- 在专用线程运行 watcher，不占用 GTK 主线程；
- Primary 默认开启；
- watcher 支持运行中关闭 Primary 记录。

相关代码：

- [clipboard.rs](../src/clipboard.rs)

### 2.3 文本规则

已实现：

- 基础 MIME 类型存在 `text/plain` 才记录；
- `text/plain;charset=utf-8` 等带参数形式满足门槛；
- 保存同一次事件中全部 `text/*`；
- 只保存成功读取的表示；
- `text/plain` 读取失败时整条跳过；
- 辅助 MIME 读取失败时忽略该表示；
- 使用声明 charset 解码预览文本；
- `text/plain` trim 后为空时不记录；
- 单条记录所有 `text/*` 合计超过 4 MiB 时整条跳过；
- 基于完整 MIME 集合和原始字节生成 SHA-256 fingerprint；
- 相同 fingerprint 去重；
- 相同 `text/plain` 但 MIME 集合不同视为不同记录。

相关代码：

- [domain/model.rs](../src/domain/model.rs)

### 2.4 菜单预览

已实现：

- 移除预览内容中的全部空白字符；
- 按 Unicode grapheme cluster 计算 15 个字符；
- 超长时追加省略号；
- 只影响菜单预览，不改变原始内容、搜索内容或恢复内容。

相关代码：

- [domain/preview.rs](../src/domain/preview.rs)

### 2.5 系统托盘

已实现：

- StatusNotifierItem 注册；
- 左键激活时显示菜单；
- Primary 条目显示在普通历史之前；
- Primary 与普通历史之间的条件分割线；
- 普通历史动态菜单；
- 菜单条目恢复剪贴板；
- “展示全部记录”动作；
- 托盘菜单“清空历史”立即清空 SQLite 普通历史；
- “退出”动作；
- 通过 tray handle 触发菜单布局更新。

“配置”菜单现在会打开 GTK 配置窗口。

相关代码：

- [tray.rs](../src/tray.rs)

### 2.6 GTK 窗口

已实现基础窗口：

- 从托盘动作打开“全部记录”窗口；
- 显示普通历史纯文本内容；
- 点击条目恢复完整 MIME 集合并移动到顶部；
- 窗口保持打开；
- 关闭窗口时隐藏并复用实例；
- 窗口不展示 Primary。

搜索、行尾叉号删除、底部清空按钮和清空确认窗口已经实现。

相关代码：

- [ui.rs](../src/ui.rs)

### 2.7 自身写入抑制

已实现：

- 恢复剪贴板前记录本次 fingerprint；
- 自身 selection 事件重新读取 payload 后比较 fingerprint；
- 匹配时忽略，不新增记录、不再次排序、不重复持久化；
- 抑制标记在超时后自动失效；
- 用户之后从其他应用复制相同内容仍可正常处理。

### 2.8 单实例

已实现：

- `GtkApplication` 使用固定 application ID；
- 第二次启动进程立即退出，不创建第二个监听器或托盘图标；
- 激活转发路径连接到现有实例；
- 现有实例收到激活后打开“全部记录”窗口。

### 2.9 测试和辅助工具

- 32 个领域层、存储层、剪贴板、托盘、UI 和 i18n 单元测试；
- `cargo test` 通过；
- `cargo clippy --all-targets -- -D warnings` 通过；
- 多 MIME 辅助源可同时提供：
  - `text/plain`
  - `text/plain;charset=utf-8`
  - `text/html`
  - `text/rtf`

相关文件：

- [multi_mime_source.rs](../examples/multi_mime_source.rs)

### 2.10 SQLite / SQLx 持久化

已实现：

- 默认数据库路径：
  - `$XDG_DATA_HOME/clipman-rs/clipman.sqlite3`
  - 未设置时使用 `~/.local/share/clipman-rs/clipman.sqlite3`
- SQLx SQLite 连接池；
- 嵌入式 migration；
- 普通历史与 MIME 表示表；
- Primary 单行记录与表示表；
- settings 单行表；
- 单写 StorageActor；
- WAL、foreign keys 和 busy timeout；
- 新增历史事务化写入；
- MIME BLOB 持久化；
- fingerprint 唯一约束和去重；
- `last_used_seq` 持久化最近使用顺序；
- 恢复条目后持久化置顶；
- 清空普通历史持久化；
- 最大记录数缩小后持久化淘汰；
- 启动时加载普通历史、Primary、顺序和设置；
- Primary 启用状态加载后应用到剪贴板服务。

相关文件：

- [storage.rs](../src/storage.rs)
- [0001_initial.sql](../migrations/0001_initial.sql)

### 2.11 配置窗口

已实现：

- 托盘“配置”菜单打开 GTK 配置窗口；
- Primary Selection 开关；
- 最大记录数 SpinButton；
- 菜单最大展示数 SpinButton；
- 菜单展示数自动限制为不超过最大记录数；
- 修改立即通过 StorageActor 持久化；
- 配置变化完成后刷新托盘菜单和窗口；
- Primary 关闭时在同一数据库事务中清除 Primary 记录；
- Primary 配置变更后同步到剪贴板 watcher；
- Primary 不受支持时开关自动禁用。

### 2.12 完整记录窗口

已实现：

- 搜索输入框；
- 按 `text/plain` 做不区分大小写的包含过滤；
- 搜索结果实时刷新；
- 搜索无结果提示；
- 点击普通历史恢复并持久化置顶；
- 鼠标悬停行尾显示叉号删除按钮；
- 点击叉号直接删除单条记录并刷新数据库快照；
- 窗口底部“清空历史”按钮；
- 清空前显示 GTK 确认对话框；
- 清空只删除普通历史，不删除 Primary；
- 窗口关闭时隐藏并复用。

### 2.13 国际化

已实现：

- 自包含英文和简体中文 JSON 资源；
- 根据 `LC_ALL`、`LC_MESSAGES`、`LANG` 选择 locale；
- 以 `zh` 开头的 locale 使用简体中文；
- 其他语言回退英文；
- 托盘菜单、历史窗口、配置窗口、确认框和错误提示统一使用翻译资源；
- 剪贴板内容和搜索内容不翻译。

相关文件：

- [i18n.rs](../src/i18n.rs)
- [en.json](../locales/en.json)
- [zh_CN.json](../locales/zh_CN.json)

### 2.14 MIME 读取超时

已实现：

- 使用 rustix `poll` 等待 pipe 可读；
- 单个 MIME 读取 deadline 为 2 秒；
- 单个表示最多读取 4 MiB；
- 超时或超过上限时返回错误，不阻塞 watcher 线程；
- 单元测试覆盖完整读取、超时和大小限制。

### 2.15 应用身份与打包

已实现：

- 稳定 application ID：`io.github.clipman_rs.Clipman`；
- SVG 应用图标；
- desktop 文件；
- 用户级安装脚本；
- release 动态链接构建；
- desktop 文件格式验证；
- SVG 图标渲染验证；
- 临时目录安装验证。

相关文件：

- [desktop 文件](../packaging/io.github.clipman_rs.Clipman.desktop)
- [SVG 图标](../packaging/icons/hicolor/scalable/apps/io.github.clipman_rs.Clipman.svg)
- [安装脚本](../scripts/install-user.sh)
- [打包说明](../packaging/README.md)

## 3. 已实测的集成场景

当前环境为 labwc 0.20.2 和 Waybar。

已验证：

1. `ksni` 图标成功出现在 Waybar 托盘。
2. 普通剪贴板复制内容写入 SQLite 并显示在菜单中。
3. Primary Selection 复制内容进入菜单顶部。
4. Primary 与普通历史之间正确显示分割线。
5. 四 MIME 条目被完整捕获。
6. 恢复后 `text/plain`、`text/plain;charset=utf-8`、`text/html` 和 `text/rtf` 重新出现在剪贴板。
7. 目标消费者可以读取恢复后的 `text/plain` 和 `text/html`。
8. 恢复普通剪贴板和 Primary 内容成功。
9. 自身事件被抑制，没有循环新增或重复菜单项。
10. 菜单预览移除空白并截断到 15 grapheme。
11. 托盘“清空历史”立即清除普通历史。
12. “展示全部记录”窗口可以由托盘动作打开。
13. 第二次启动没有产生第二个后台进程。
14. 使用临时 `XDG_DATA_HOME` 启动应用后，SQLite 文件和 migration 正常创建。
15. 普通历史、Primary、MIME BLOB 和顺序成功写入数据库。
16. 退出并重新启动后，普通历史、Primary、顺序和设置从数据库恢复。
17. 数据库重启后，Primary 条目继续显示在托盘菜单顶部。
18. 托盘“配置”动作成功打开配置窗口，并显示当前数据库中的 Primary 和数量设置。
19. 完整记录窗口成功显示搜索框、历史列表、悬停删除入口和底部清空按钮。
20. 搜索大小写行为和数量约束由单元测试覆盖。
21. 单条删除和关闭 Primary 清除记录的数据库行为由存储测试覆盖。
22. 关闭普通剪贴板源进程后，clipman-rs 自动接管 selection，文本仍可直接粘贴。
23. 清空普通历史后，当前普通剪贴板同步清空，`wl-paste` 返回空。
24. 普通历史为空时，托盘菜单显示不可点击的“暂无记录”。
25. “全部记录”和“配置”窗口同时打开时均稳定显示一致的 GTK HeaderBar。
26. 英文 locale 下托盘和窗口显示英文。
27. 中文 locale 下托盘和窗口显示简体中文。
28. MIME pipe 超时和大小上限由单元测试覆盖。
29. Waybar 重启后，新 StatusNotifierWatcher 自动接收到 clipman-rs SNI。
30. 选择历史后退出 clipman-rs，普通剪贴板内容随数据源退出而失效。
31. 用户级安装脚本成功安装 release 二进制、desktop 文件和 SVG 图标。
32. desktop 文件通过 `desktop-file-validate`，SVG 可正常渲染为 128×128 PNG。

## 4. 已加入但尚未完整验收

### 4.1 GTK 交互自动化

配置和历史窗口已经实现，但 GTK 控件交互仍主要依赖人工验证。当前没有自动化测试覆盖：

- 鼠标悬停后才显示删除按钮；
- 点击叉号后的窗口实时刷新；
- 清空确认对话框的两个分支；
- Primary Switch、SpinButton 和值约束的完整信号链；
- 窗口隐藏后重新打开的状态。

底层数据库操作、搜索过滤和数量约束已有单元测试覆盖。

## 5. 待实现

### 阶段 1：应用骨架与持久化

- [x] SQLite 数据目录和数据库初始化；
- [x] SQLx migration；
- [x] 普通历史表和 MIME 表示表；
- [x] Primary 单行表和表示表；
- [x] settings 表；
- [x] StorageActor / repository；
- [x] 启动时加载历史和配置；
- [x] 新增、排序和清空即时落盘；
- [x] 数据库重启恢复测试。

### 阶段 2：历史与 Primary 完整行为

- [x] 最大存储记录数配置和持久化；
- [x] 菜单最大展示数配置和持久化；
- [x] 调小最大记录数时立即淘汰较旧记录；
- [x] Primary 开关配置持久化；
- [x] 关闭 Primary 时清除持久化 Primary 记录；
- [x] 启动后只加载历史，不恢复系统剪贴板。

### 阶段 3：托盘完整交互

- [x] Primary 条件分割线；
- [x] 最近普通历史动态更新；
- [x] 托盘点击恢复；
- [x] 托盘“清空历史”不确认立即执行；
- [x] “展示全部记录”入口；
- [x] “配置”菜单打开配置窗口；
- [x] Waybar 重启后的托盘恢复验证；
- [x] 退出语义和剪贴板数据源失效验证。

### 阶段 4：窗口和配置

- [x] 搜索框；
- [x] 按 `text/plain` 做不区分大小写包含过滤；
- [x] 行尾叉号单条删除；
- [x] 单条删除立即持久化；
- [x] 窗口底部“清空历史”按钮；
- [x] 清空历史确认窗口；
- [x] 配置窗口；
- [x] Primary 开关；
- [x] 最大记录数和菜单展示数 SpinButton；
- [x] 配置即时生效和保存。

### 阶段 5：完善与发布

- [x] 自包含 JSON 中文/英文；
- [x] 用户可见错误 i18n；
- [x] MIME 读取超时；
- [x] 异常剪贴板源和管道阻塞处理；
- [ ] Firefox 等真实富文本来源验证；
- [ ] 常见编辑器中 `Ctrl+V` / `Ctrl+Shift+V` 验证；
- [ ] XWayland 与 Wayland 双向复制验证；
- [x] 应用图标和最终 application ID；
- [x] desktop 文件和打包说明；
- [ ] 完整手工验收。

## 6. 当前不作为 MVP 的功能

- 图片和截图图片；
- 文件对象；
- `application/*` MIME；
- 固定条目；
- 暂停记录；
- 全局快捷键；
- 自动粘贴或模拟按键；
- 按源应用排除；
- 时间过期策略；
- 专用 Waybar 模块；
- 管理桌面 autostart；
- 独立 AppImage 或 Flatpak。

## 7. 已知限制与风险

### 7.1 真实应用兼容性尚未验收

已经用 data-control 测试源验证四 MIME，但尚未验证：

- Firefox 或 Chromium 的富文本 MIME 集合；
- LibreOffice 等编辑器的格式选择；
- XWayland 应用和 Wayland 应用之间的双向复制；

### 7.2 UI 仍是原型

- GTK 交互缺少自动化测试；
- 搜索结果和长文本排版未做视觉细化；
- 样式没有针对 Waybar/GTK 主题做细化；

### 7.3 单实例激活窗口只验证了部分路径

第二次启动不会创建第二实例，这一点已验证。现有窗口关闭后，再次启动是否在所有 labwc 焦点状态下稳定呈现窗口，仍需补充验收。

## 8. 已记录问题状态

- `KI-001`：已修复；两个窗口统一使用显式 GTK HeaderBar。
- `KI-002`：已修复；普通历史为空时显示不可点击的“暂无记录”。
- `KI-003`：已修复；源应用退出后由 clipman-rs 接管普通剪贴板。
- `KI-004`：已修复；清空普通历史时同步清除普通剪贴板。

详细信息见 [known-issues.md](known-issues.md)。

## 9. 下一步

建议进入阶段 5：

1. 验证 Firefox、Chromium 中复制内容及常见编辑器的粘贴行为；
2. 验证 XWayland 与 Wayland 双向复制；
3. 增加 GTK 交互自动化或完整人工验收记录；
4. 完成最终 MVP 验收检查。

