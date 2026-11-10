# clipman-rs 技术设计与架构方案

> 状态：主要功能与发布资产已完成，进入最终兼容性验收  
> 日期：2026-10-27  
> 需求基线：[requirements.md](requirements.md)  
> 实现进度：[status.md](status.md)

## 1. 文档目的

本文档记录 clipman-rs MVP 的技术选型依据、总体架构、核心数据流、模块职责、并发模型和验证计划。

功能行为的最终依据仍然是 [requirements.md](requirements.md)。本文档关注这些行为如何用 Rust、GTK4、Wayland data-control、StatusNotifierItem 和 SQLite 实现。

## 2. 功能目标

MVP 提供一个运行于 labwc/Wayland 的独立文本剪贴板历史管理器：

- 监听普通剪贴板和 Primary Selection；
- 仅当剪贴板事件提供 `text/plain` 时记录；
- 保存同一次事件中全部 `text/*` MIME 表示；
- 通过系统托盘菜单浏览、恢复和清空最近历史；
- 通过独立窗口搜索、删除和清空全部记录；
- 提供 Primary Selection 开关和两个历史数量配置；
- 持久化普通历史、Primary 记录、顺序和配置；
- 作为标准系统托盘程序与 Waybar 解耦；
- 保持单实例运行；
- 支持国际化；
- 不记录图片、文件对象或 `application/*` 内容。

## 3. 技术路线

MVP 采用以下组合：

> **Rust + GTK4/gtk4-rs + wl-clipboard-rs data-control + ksni + SQLx/SQLite + Tokio**

| 领域 | 选择 |
|---|---|
| 编程语言 | Rust |
| GUI | GTK4 + `gtk4-rs` |
| 普通剪贴板 | `wl-clipboard-rs` + `ext-data-control` / `wlr-data-control` |
| Primary Selection | `wl-clipboard-rs` data-control primary selection |
| 系统托盘 | `ksni`，StatusNotifierItem + D-Bus Menu |
| 数据库 | SQLite |
| 数据库访问 | SQLx |
| 异步运行时 | Tokio |
| 国际化 | 自包含 JSON 中英文资源 |
| 单实例 | `GtkApplication` application ID |
| GUI 扩展库 | 首版不引入 libadwaita |

## 4. 选型依据

### 4.1 Rust

Rust 适合本项目对剪贴板生命周期、多线程数据传递和并发状态管理的要求。核心模型、MIME 处理、持久化和托盘逻辑都保持在同一个语言和测试体系中。

### 4.2 为什么选择 GTK4

项目比较过 GTK4 和 Qt 两条路线。

Qt 的主要优势是 `QSystemTrayIcon`、`QMenu` 和 `QClipboard` 原生可用。但当前重点评估的 `qtbridge-rust` 主要面向 Qt Quick/QML：

- Rust 类型通过 `#[qobject]` 暴露给 QML；
- 当前公开类型库没有封装 `QClipboard`、`QMimeData`、`QSystemTrayIcon` 和 `QMenu`；
- 官方 README 明确建议涉及 Qt Widgets 或仅提供 C++ API 的模块时使用 CXX-Qt；
- 项目版本仍较早，API 有过 breaking change；
- 项目未启用 GitHub Issues，外部问题检索和反馈渠道有限。

QML 的 `Qt.labs.platform` 可以完成托盘和菜单，但通用多 MIME 剪贴板仍需要额外 CXX/CXX-Qt 桥接层。继续使用 Qt 会同时引入 QML、C++ 桥接和较新的 QtBridge 依赖。

GTK4 路线则可以保持：

- GUI 使用 Rust；
- 普通剪贴板和 Primary Selection 使用 `wl-clipboard-rs` 的 data-control 接口；
- 数据、持久化和异步任务使用 Rust；
- 托盘使用 `ksni` 提供的现成 SNI 封装。

本项目是界面相对简单的工具型应用，主要复杂度在剪贴板和多 MIME，而不是复杂视觉效果，因此减少桥接层比使用 Qt Quick 更有利。

### 4.3 为什么使用 data-control 而不是 GDK 后台监听

阶段 0 实测发现，普通 Wayland data-device 的 selection 事件主要面向获得键盘焦点的客户端。GTK/GDK 即使在后台持有 `GdkClipboard`，也不能可靠监听其他应用的全局复制事件。这与剪贴板管理器的核心需求冲突。

labwc/wlroots 同时提供 `ext-data-control-v1` 和 `wlr-data-control-unstable-v1`。这两个协议专为剪贴板管理器等无窗口客户端设计，可以在没有键盘焦点时观察普通剪贴板和 Primary Selection。

`wl-clipboard-rs` 已封装：

- 协议选择与 Wayland 连接；
- 普通剪贴板和 Primary Selection 的持续监听；
- data offer MIME 枚举；
- 通过 pipe 读取每个 MIME；
- 使用 `copy_multi` 同时提供多个 MIME 表示；
- 读取取消和错误处理。

因此实际方案是：

- GTK4 只负责窗口和控件；
- `wl-clipboard-rs` 负责剪贴板；
- `ksni` 负责系统托盘；
- SQLx/Tokio 负责持久化。

直接实现 Wayland 剪贴板需要自行处理：

- `wl_data_device`、`wl_data_offer`、`wl_data_source`；
- 每次选区变化后的 pipe 读取和生命周期；
- 多 MIME 的异步读取和取消；
- 作为数据源响应目标应用的发送请求；
- `zwp_primary_selection_device_v1`；
- XWayland 与 Wayland 之间的选择和剪贴板桥接；
- 自身写入事件抑制。

`wl-clipboard-rs` 已覆盖这些协议细节，因此首版不直接编写 data-control 协议状态机。

### 4.4 为什么使用 ksni

GTK4 本身不提供系统托盘 API，因此使用纯 Rust 的 `ksni` 封装 StatusNotifierItem。

当前 `ksni` 已支持本项目需要的功能：

- StatusNotifierItem 和 D-Bus Menu；
- `MENU_ON_ACTIVATE`，左键激活时直接打开菜单；
- `StandardItem`、`CheckmarkItem` 和 `SubMenu`；
- `MenuItem::Separator`；
- `menu_about_to_show()`；
- 通过 tray handle 动态更新菜单；
- Tokio、async-io 和 blocking 三种使用方式。

这避免了应用程序自行实现 D-Bus 接口、菜单布局更新和 SNI 注册流程。

### 4.5 为什么使用 SQLx + SQLite

SQLx 提供 SQLite 支持、连接池、事务和 migration，适合保存少量但包含多种 MIME BLOB 的文本历史。

SQLx 的异步 API 与 `ksni` 的 Tokio 运行时可以共用同一个后台运行时。QtBridge 的 `host_monitor` 示例也验证了“后台 Tokio 执行异步任务、回到 GUI 线程更新界面”这一模式；本项目采用相同原则，但使用 GTK 主循环。

首版优先使用 SQLx 运行时查询 API，避免编译期查询宏对 `DATABASE_URL` 或 SQLx 离线元数据的额外构建依赖。schema 稳定后再评估是否引入编译期查询检查。

## 5. 总体架构

```text
┌─────────────────────────────────────────────────────────────┐
│                         GTK 主线程                           │
│                                                             │
│  GtkApplication       历史窗口          配置窗口             │
│       │                  │                 │                │
│       └──────────── UI Controller ──────────────────────────┤
└───────────────┬───────────────────────┬─────────────────────┘
                │                       │
       捕获事件与恢复命令          快照与托盘命令
                │                       │
┌───────────────▼─────────────┐ ┌───────▼─────────────────────┐
│ data-control 剪贴板线程      │ │ Tokio 后台运行时             │
│                             │ │                             │
│ Watcher / copy_multi        │ │ History Service             │
│ CLIPBOARD / PRIMARY         │ │ Storage Actor / SQLx        │
│                             │ │ Tray / ksni                 │
└─────────────────────────────┘ └─────────────────────────────┘
```

### 5.1 主要组件

| 组件 | 职责 |
|---|---|
| `GtkApplication` | 应用生命周期、单实例、窗口激活 |
| `ApplicationCoordinator` | 串行处理 UI、托盘和剪贴板事件 |
| `ClipboardService` | data-control 监听、读取、MIME 过滤、恢复和自身事件抑制 |
| `HistoryService` | 文本判定、去重、排序、数量限制和快照生成 |
| `StorageActor` | SQLx/SQLite 事务、查询、migration 和持久化 |
| `TrayService` | ksni 图标、动态菜单和菜单点击事件 |
| `HistoryWindow` | 全部记录、搜索、单条删除和清空确认 |
| `SettingsWindow` | Primary 开关和两个最大数量配置 |
| `LocaleService` | 根据系统 locale 选择并加载嵌入式中英文资源 |

## 6. 线程与事件模型

### 6.1 GTK 主线程

GTK 对象只能在其创建线程使用，因此 GTK 主线程负责：

- 所有 GTK 窗口和控件；
- 用户界面事件和快照渲染；
- 应用退出流程。

剪贴板不依赖 GTK 主线程。`ClipboardService` 在专用线程中阻塞等待 data-control 事件并读取 offer。

### 6.2 Tokio 后台运行时

Tokio runtime 负责：

- `ksni` 托盘服务和 D-Bus 事件；
- SQLx 查询和事务；
- 从托盘线程向 GTK 主线程转发操作；
- 从数据库返回结果或快照到 GTK 主线程。

后台线程不得直接操作 GTK 对象。剪贴板捕获结果通过 channel 投递到 GTK 主线程；显示窗口或退出应用也必须通过 channel 交给 GTK 主线程。

### 6.3 状态所有权

建议由单一应用协调器串行处理以下事件：

```text
ClipboardCaptured
ClipboardRestoreRequested
HistoryEntryActivated
HistoryEntryDeleted
HistoryCleared
PrimaryEnabledChanged
MaxEntriesChanged
MenuEntriesChanged
ShowHistoryRequested
ShowSettingsRequested
ExitRequested
```

所有历史修改通过 `HistoryService` 和 `StorageActor` 串行执行，避免：

- 托盘菜单和窗口看到不同顺序；
- 两个异步任务同时更新同一记录；
- 自身写入和外部复制发生竞争；
- SQLite 写事务互相覆盖。

### 6.4 快照

数据库提交成功后生成只读 `AppSnapshot`，至少包含：

- Primary 条目元数据；
- 普通历史条目元数据；
- 当前配置；
- 普通历史总数；
- 数据版本号。

GTK UI 和托盘菜单都从同一份快照渲染。这样无需在两个线程同时维护可变业务状态。

## 7. 数据模型与持久化

### 7.1 数据目录

使用 XDG 数据目录保存数据库：

```text
$XDG_DATA_HOME/clipman-rs/clipman.sqlite3
```

未设置 `$XDG_DATA_HOME` 时使用：

```text
~/.local/share/clipman-rs/clipman.sqlite3
```

数据库属于持久数据，不放在缓存目录。导出路径不需要在配置窗口展示。

### 7.2 逻辑表结构

#### `history_entries`

- `id`：整数主键；
- `fingerprint`：完整表示集合的稳定哈希，唯一；
- `plain_text`：用于预览和搜索的解码文本；
- `captured_at`：首次复制时间；
- `last_used_at`：最近复制或从历史选择的时间。

#### `history_representations`

- `entry_id`：外键；
- `mime_type`：完整 MIME 字符串，包括参数；
- `data`：原始字节 BLOB；
- 主键为 `(entry_id, mime_type)`。

删除历史条目时级联删除对应表示。

#### `primary_entry`

单行表，约束为固定主键 `1`：

- `fingerprint`；
- `plain_text`；
- `captured_at`。

#### `primary_representations`

- `mime_type`；
- `data`；
- 与 `primary_entry` 关联。

#### `settings`

保存：

- `primary_enabled`；
- `max_history_entries`；
- `max_menu_entries`。

首次启动默认值：

```text
primary_enabled = true
max_history_entries = 20
max_menu_entries = 10
```

写入前验证：

```text
1 <= max_menu_entries <= max_history_entries
```

### 7.3 SQLite 配置

- 启用 WAL；
- 启用 foreign keys；
- 设置合理的 busy timeout；
- 所有复合写入使用事务；
- schema 变更通过 SQLx migration 管理；
- 数据目录不存在时自动创建。

### 7.4 数据写入策略

以下操作完成后立即提交持久化：

- 新增历史；
- 重复历史移动到顶部；
- 从历史中选择条目；
- 删除单条记录；
- 清空普通历史；
- Primary 内容更新；
- Primary 开关关闭并清除；
- 配置修改；
- 最大记录数缩小导致淘汰。

数据库提交成功后再发布新的 UI/托盘快照。

### 7.5 启动与退出

启动过程：

1. 初始化 GTK Application；
2. 建立 Tokio runtime 和 SQLx 连接；
3. 执行 migration；
4. 读取设置、普通历史和 Primary 记录；
5. 启动托盘；
6. 开始监听剪贴板；
7. 不把历史内容写回系统剪贴板。

退出过程：

1. 停止剪贴板监听；
2. 停止托盘服务；
3. 停止接收新业务事件；
4. 结束 Tokio runtime；
5. 结束 GTK 主循环；
6. 保留历史、Primary 记录和配置。

如果当前普通剪贴板由本程序提供，进程退出后数据源失效，符合需求。

## 8. 剪贴板捕获设计

### 8.1 监听来源

- 通过 `wl_clipboard_rs::watch::Watcher` 监听 `ClipboardType::Both`；
- 每个事件通过 `paste::ClipboardType` 区分普通剪贴板和 Primary Selection；
- Primary 监听是否生效由共享的启用状态控制；
- watcher 启动时产生的当前选区状态事件被跳过，不主动采集已有选区。

监听运行在专用 Rust 线程中，不依赖 GTK 事件循环或键盘焦点。

### 8.2 捕获流程

```text
data-control selection 事件
  ↓
确认不是自身写入
  ↓
枚举当前 MIME
  ↓
筛选所有 text/*
  ↓
确认存在基础类型 text/plain
  ↓
逐个通过 pipe 读取 text/* 表示
  ↓
解码 text/plain 并 trim 判定空内容
  ↓
检查所有表示合计大小 <= 4 MiB
  ↓
计算完整表示集合 fingerprint
  ↓
提交 StorageActor
  ↓
去重、更新时间、数量淘汰
  ↓
发布 AppSnapshot
  ↓
刷新托盘菜单和打开的窗口
```

### 8.3 记录门槛

只有存在 `text/plain` 基础媒体类型时才记录：

- `text/plain`、`text/plain;charset=UTF-8` 等形式都满足门槛；
- 只有 `text/html` 而没有 `text/plain` 时不记录；
- 只有 `text/uri-list` 而没有 `text/plain` 时不记录；
- 程序不进行内容嗅探。

满足门槛后，同一次事件中所有读取成功的 `text/*` 都进入同一个历史条目。

### 8.4 MIME 读取策略

- `text/plain` 必须读取成功，否则整条跳过；
- 其他 `text/*` 读取失败或超时则忽略该表示；
- 每次读取通过 poll/read deadline 限制在 2 秒内；
- 每次读取限制在 4 MiB 上限内；
- 超时、损坏管道或异常源不会永久阻塞 watcher 线程；
- 失败的其他 MIME 不生成占位数据。

### 8.5 文本解码

原始 `text/plain` 字节必须保留用于恢复。另生成解码后的 `plain_text` 用于：

- trim 后空内容判断；
- 菜单预览；
- 全部记录列表；
- 搜索。

解码优先使用 MIME 中声明的 charset；没有声明时使用 UTF-8。对常见编码使用 `encoding_rs`，无法可靠解码时使用 UTF-8 lossy 预览，但保存和恢复仍使用原始字节。

### 8.6 fingerprint 与去重

fingerprint 由以下内容稳定计算：

- 所有保存的 MIME 字符串；
- 每个 MIME 的原始字节；
- MIME 与字节按 MIME 字符串排序后拼接；
- 对规范化结果计算 SHA-256。

只有完整表示集合相同才视为重复：

- 重复时更新 `last_used_at` 并移动到顶部；
- `text/plain` 相同但 MIME 集合或内容不同，视为新记录。

### 8.7 容量控制

- 菜单预览使用 Unicode grapheme cluster，而不是字节；
- 普通历史最大条数和菜单展示最大条数从配置读取；
- 新增记录或调小最大条数后，只保留最近指定数量的普通记录；
- Primary 记录不参与普通历史数量限制。

## 9. 剪贴板恢复设计

### 9.1 恢复流程

```text
用户点击 Primary 或普通历史
  ↓
从 StorageActor 读取全部 MIME 表示
  ↓
使用 wl-clipboard-rs copy_multi 为每个 MIME 创建数据源
  ↓
标记本次为程序自身写入
  ↓
通过 data-control 设置普通剪贴板 selection
  ↓
普通历史条目移动到顶部并持久化
```

程序只把内容写入普通剪贴板，不模拟 `Ctrl+V`。

从托盘菜单选择时，菜单关闭；从“展示全部记录”窗口选择时，窗口保持打开。

### 9.2 自身写入抑制

每次程序主动设置普通剪贴板时：

1. 记录本次写入的 fingerprint 和来源条目；
2. 通过 `copy_multi` 设置剪贴板内容；
3. 收到由本次写入触发的 selection 事件时，重新读取 payload 并比较 fingerprint；
4. 匹配则忽略，不新增记录、不再次置顶、不再次写数据库；
5. 清除本次抑制标记；未消费的标记在短时间后过期。

抑制标记不能永久忽略相同内容。用户之后从其他应用再次复制相同内容时，仍必须正常处理和置顶。

### 9.3 Primary 恢复

点击 Primary 条目时只写入普通剪贴板，不修改 Primary Selection。

Primary 开关关闭时：

1. 停止 Primary 监听；
2. 删除持久化的 Primary 条目；
3. 发布不含 Primary 的新快照；
4. 托盘菜单不再展示 Primary 区域及其分割线。

## 10. 托盘设计

### 10.1 ksni Tray

`TrayService` 实现 `ksni::Tray`：

- 使用稳定且唯一的 tray ID；
- 使用主题图标或内嵌图标；
- 设置 `MENU_ON_ACTIVATE = true`；
- `menu_about_to_show()` 时确保使用最新快照；
- 业务状态变化时通过 tray handle 更新菜单。

### 10.2 菜单生成

菜单从 `AppSnapshot` 纯函数生成：

```text
Primary 条目（启用且存在时）
--------------------
最近 N 条普通历史
--------------------
展示全部记录
清空历史
配置
退出
```

第一条分割线仅在 Primary 区域实际显示时出现。

菜单条目：

- 使用 `text/plain` 预览；
- 移除预览内容中的所有空白字符，包括首尾和内部的空格、换行、制表符等；
- 从移除空白后的内容中取前 15 个 Unicode grapheme cluster；
- 超长时追加省略号；
- 原始内容、搜索结果和恢复内容不受预览清理影响；
- 15 是显示层常量，不在配置窗口暴露。

### 10.3 菜单动作

每个历史菜单项捕获具体条目 ID，而不是只按列表下标处理。点击后向 GTK 主线程发送恢复命令。

其他动作：

| 动作 | 行为 |
|---|---|
| 展示全部记录 | 发送 `ShowHistoryRequested` |
| 清空历史 | 不确认，立即清空普通历史 |
| 配置 | 发送 `ShowSettingsRequested` |
| 退出 | 发送 `ExitRequested` |

托盘线程不得直接操作 GTK 或 SQLite。

## 11. GTK 窗口设计

### 11.1 全部记录窗口

窗口组成：

- 顶部 `GtkSearchEntry`；
- 中部可滚动历史列表；
- 底部“清空历史”按钮。

列表实现优先使用：

- `GtkListBox`：每行一个自定义历史项；
- `GtkFilterListModel` + `GtkCustomFilter`：按 `plain_text` 做不区分大小写包含过滤；
- `GtkScrolledWindow`：承载列表。

每个历史行：

- 显示 `text/plain` 内容；
- 点击行恢复完整 MIME 集合并移动到顶部；
- 窗口保持打开；
- 鼠标悬停时显示行尾叉号按钮；
- 点击叉号直接删除单条记录，无额外确认。

“清空历史”：

- 点击后打开 GTK 确认对话框；
- 确认后清空全部普通历史，而不只是当前搜索结果；
- Primary 不受影响。

该窗口不展示 Primary。

### 11.2 配置窗口

配置项：

- `GtkSwitch`：记录 Primary Selection；
- `GtkSpinButton`：最大存储记录数；
- `GtkSpinButton`：托盘菜单最大展示数。

交互规则：

- 修改立即生效并持久化；
- 没有应用或取消按钮；
- 菜单最大展示数始终受最大存储记录数约束；
- 调小最大存储记录数后立即淘汰较旧普通记录；
- Primary 关闭时立即清除 Primary 数据。

配置窗口可以关闭，但不会销毁应用。

### 11.3 窗口生命周期

应用作为托盘后台程序运行，关闭窗口不等于退出应用：

- 全部记录窗口和配置窗口各保持一个应用级实例；
- 关闭窗口时隐藏可视窗口，下次打开时复用；
- 托盘进程继续运行；
- 只有“退出”结束应用。

如果使用 `GtkApplication` 的自动退出逻辑，需要在托盘运行期间保持 application hold，避免最后一个窗口关闭时进程退出。

## 12. 单实例与应用生命周期

使用 `GtkApplication` 和稳定的 application ID 提供会话 D-Bus 单实例能力：

- 第一个进程注册 application ID；
- 后续启动不会创建新的监听器和托盘图标；
- 第二实例把激活请求转交给已有实例；
- 现有实例收到激活请求后打开“展示全部记录”窗口。

稳定 application ID 为 `io.github.clipman_rs.Clipman`。desktop 文件和 SVG 图标已按该 ID 提供。

应用不管理 autostart。用户通过 labwc 或系统会话配置决定是否开机启动。

### 12.1 打包策略

MVP 采用系统 GTK4 动态链接：

- 开发阶段只要求系统提供 GTK4 运行库；
- 首版不制作 AppImage 或 Flatpak；
- 提供用户级安装脚本、desktop 文件和 hicolor SVG 图标；
- SQLx、ksni 等 Rust 依赖由 Cargo 构建；
- 程序稳定后再评估是否需要独立打包运行时。

## 13. 国际化

使用编译进程序的自包含 JSON 翻译资源：

- `locales/en.json` 提供英文和回退文案；
- `locales/zh_CN.json` 提供简体中文；
- 根据 `LC_ALL`、`LC_MESSAGES`、`LANG` 的顺序检测 locale；
- 以 `zh` 开头的 locale 使用简体中文，其他语言回退英文；
- GTK、托盘菜单、配置项、确认对话框和错误提示统一通过 `Translator` 取文本；
- 菜单预览和搜索内容不翻译；
- 首版不要求运行中动态切换语言。

## 14. 错误处理与日志

建议使用 `tracing` 输出结构化日志。

需要记录但不导致程序退出的错误：

- 单个非 `text/plain` MIME 读取失败；
- 剪贴板源提前失效；
- 剪贴板读取超时；
- SQLite 临时忙；
- 托盘服务暂时不可用；
- 翻译文件缺失。

需要跳过的捕获：

- `text/plain` 无法读取；
- trim 后为空；
- 表示总大小超过 4 MiB。

数据库 migration 或启动加载失败属于致命错误，应明确提示用户，不能以空数据库静默覆盖原数据。

用户可见错误需要支持 i18n。

## 15. 模块划分

建议目录：

```text
src/
  main.rs
  app.rs
  events.rs
  domain/
    model.rs
    history.rs
    preview.rs
  clipboard/
    capture.rs
    restore.rs
    mime.rs
  storage/
    mod.rs
    repository.rs
    service.rs
  tray/
    mod.rs
    menu.rs
  ui/
    history_window.rs
    settings_window.rs
  i18n.rs
migrations/
locales/
resources/
  icons/
docs/
  requirements.md
  design.md
```

剪贴板、历史规则和存储应尽量避免直接依赖 GTK 控件，便于单元测试和替换。

## 16. 预计依赖

核心依赖方向：

- `gtk4`、`glib`、`gio`
- `ksni`
- `tokio`
- `sqlx`，启用 SQLite、Tokio runtime 和 migration
- `serde`
- `serde_json`，读取嵌入式翻译资源
- `sha2`
- `encoding_rs`
- `unicode-segmentation`
- `rustix`，实现带 deadline 的 pipe poll/read
- `thiserror`
- `tracing`
- `tracing-subscriber`
- XDG 目录解析库

精确版本和 feature 在技术验证完成后锁定，并以 `Cargo.lock` 为准。

## 17. 测试策略

### 17.1 单元测试

- `text/plain` 基础类型识别；
- trim 后空内容跳过；
- 所有符合条件的 `text/*` 聚合；
- 4 MiB 合计大小边界；
- 文本解码和 lossy fallback；
- fingerprint 稳定性；
- 重复内容判定；
- 历史排序；
- 最大条数和菜单展示数约束；
- 移除预览中的全部空白字符；
- 15 个 grapheme cluster 预览截断；
- Primary 开关状态转换。

### 17.2 数据库测试

使用临时 SQLite 文件测试：

- migration；
- 级联删除；
- 重复内容更新顺序；
- 调小最大记录数后的即时淘汰；
- Primary 单行约束；
- 配置事务与约束；
- 重启后历史和顺序恢复；
- 单条删除和清空。

### 17.3 集成测试

- data-control 多 MIME 捕获；
- `copy_multi` 恢复；
- 自身写入事件的抑制；
- Primary Selection 更新；
- GTK UI 事件到存储快照的传播；
- 托盘动作到 GTK 主线程的调度。

### 17.4 手工验收

在 labwc + Waybar 中验证：

- Waybar tray 显示图标；
- 左键打开菜单；
- Primary 和普通历史之间的分割线；
- 点击条目后菜单关闭并可粘贴；
- 全部记录窗口保持打开；
- 搜索、悬停删除和清空确认；
- Waybar 重启后托盘恢复；
- XWayland 与 Wayland 应用之间双向复制；
- 退出后历史保留，程序提供的剪贴板数据失效。

## 18. 实施前技术验证

完整开发前先做最小原型，验证以下高风险点：

1. `ksni` 在 labwc + Waybar 中显示图标、动态菜单和分割线。
2. `ksni` 的 `MENU_ON_ACTIVATE` 能让左键直接打开菜单。
3. data-control watcher 能读取真实应用提供的全部 `text/*`，并正确应用 `text/plain` 门槛。
4. `copy_multi` 能同时恢复多个 MIME，并在目标应用验证普通粘贴和纯文本粘贴。
5. data-control 能在 labwc 下监听和读取 Primary Selection。
6. fingerprint 抑制能阻止自身写入造成的循环捕获，同时不影响其他应用后续复制相同内容。
7. XWayland 与 Wayland 应用之间的文本剪贴板内容可以正常捕获和恢复。
8. GTK 主线程与 Tokio/SQLx/ksni 之间可以通过 channel 安全交互。

如果第 1 至第 6 项通过，则架构风险已经足够低，可以进入正式实现。

### 18.1 阶段 0 与阶段 1 当前实测结果

完整的已实现内容、限制和待办清单见 [status.md](status.md)。

已在当前 labwc 0.20.2 + Waybar 环境中验证：

- `ksni` 成功注册 StatusNotifierItem 到 Waybar；
- 普通剪贴板 selection 变化可被 data-control watcher 捕获；
- 包含 `text/plain`、`text/plain;charset=utf-8`、`text/html` 和 `text/rtf` 的四 MIME 条目可完整捕获；
- 恢复后四种 `text/*` 会重新出现在剪贴板，目标可正常读取 `text/plain` 和 `text/html`；
- SQLx migration 创建普通历史、MIME 表示、Primary 和 settings 表；
- 普通历史、MIME 内容、排序、Primary 和设置可跨程序重启恢复；
- 配置窗口可修改 Primary 开关和两个数量限制并即时保存；
- 完整记录窗口支持搜索、悬停单条删除和清空确认；
- 中英文资源可根据系统 locale 选择；
- MIME pipe 读取具有 2 秒超时和 4 MiB 大小限制；
- 菜单预览完成空白移除和 15 grapheme 截断；
- Primary Selection 可捕获，并显示在普通历史前的分割线上方；
- 从菜单恢复普通剪贴板和 Primary 内容成功；
- 自身写入产生的 selection 事件被 fingerprint 抑制，没有循环或重复条目；
- 第二次启动进程立即退出，没有产生第二个监听器；
- “展示全部记录”窗口可由托盘动作打开。
- Waybar 重启后，新 StatusNotifierWatcher 能重新注册 clipman-rs；
- 退出后 clipman-rs 持有的普通剪贴板数据源失效；
- release 二进制、desktop 文件和 SVG 图标可通过用户级安装脚本安装。

尚待验证：

- Firefox 等真实富文本来源的 MIME 集合；
- 多 MIME 恢复到常见编辑器后的 `Ctrl+V` / `Ctrl+Shift+V` 行为；
- XWayland 与 Wayland 双向文本剪贴板；
- 真实浏览器和 XWayland 的完整 MIME 行为。

## 19. 实施阶段

### 阶段 0：技术原型

- data-control 剪贴板捕获与恢复；
- ksni 托盘；
- 自身事件抑制；
- Primary Selection；
- Tokio/SQLx 最小集成。

### 阶段 1：应用骨架与持久化

- GtkApplication；
- 单实例；
- migration；
- 设置和历史仓库；
- 应用事件与快照。

### 阶段 2：普通历史和 Primary 捕获

- MIME 判定；
- 多 MIME 读取；
- trim、大小限制、fingerprint；
- 去重、排序和淘汰；
- Primary 开关。

### 阶段 3：托盘菜单

- 图标和菜单；
- 条件分割线；
- 15 grapheme 预览；
- 历史恢复；
- 控制项。

### 阶段 4：窗口和配置

- 全部记录窗口；
- 搜索；
- 悬停叉号删除；
- 清空确认；
- 配置即时保存。

### 阶段 5：完善

- i18n；
- 日志；
- 重启恢复；
- XWayland 验证；
- Waybar 重启和焦点测试；
- 打包和启动说明。

## 20. 已知风险

| 风险 | 影响 | 处理方式 |
|---|---|---|
| 某些应用只提供非 `text/plain` 表示 | 有效内容可能被现有门槛跳过 | 保持已确认的需求规则，按 `text/plain` 门槛执行 |
| data-control 多 MIME 恢复行为与目标应用预期不一致 | 富文本恢复可能只会使用一种格式 | 用 Firefox 和目标编辑应用验证 |
| Waybar 与 ksni 的激活语义差异 | 左键可能不直接打开预期菜单 | 验证 `MENU_ON_ACTIVATE`，必要时在 `activate` 中显式处理 |
| Primary Selection 读取依赖客户端支持 | Primary 功能可能不可用 | 检查实际 displayed clipboard 并记录运行日志 |
| 剪贴板读取期间源应用退出或管道阻塞 | 捕获失败 | 立即读取，限制大小，并使用 2 秒 poll/read deadline |
| 自身事件抑制范围过宽 | 用户后续复制丢失 | 只抑制与本次 fingerprint 对应的一次事件 |
| `text/plain` 使用非 UTF-8 charset | 预览或搜索异常 | 使用声明 charset 解码并保留原始字节 |
| SQLx 和 GTK 事件循环混用 | 卡顿或线程错误 | GTK 主线程与 Tokio 严格分工 |
| GTK 运行时依赖 | 分发体积和环境依赖增加 | 首版链接系统库，打包时明确依赖 |
| QtBridge 不可直接复用 | 未来迁移 Qt 需要额外工作 | 核心领域和存储层与 GTK 解耦 |

## 21. 文档维护

- 行为变化先更新 [requirements.md](requirements.md)。
- 架构变化同步更新本文档的选型、数据流和风险部分。
- 阶段 0 的实测结果应补充到本文档，不能只保留口头结论。
