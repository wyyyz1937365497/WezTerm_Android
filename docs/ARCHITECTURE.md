# WezTerm Android 架构与目标

最后更新：2026-09-07

首个公开版本：`v0.1.0`

WezTerm 上游基线：`d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b`

## 1. 项目定位

本项目是一个独立的、实验性的 Android 原生 WezTerm 客户端前端，不是 WezTerm
官方 Android 发行版。正确的工程目标是：

> 新建 Android 原生客户端前端，复用 WezTerm 的终端模型、字体底层、字符渲染、
> SSH 与远程 mux 客户端代码；不把桌面 `wezterm-gui` 整体交叉编译成 APK。

终端内容显示在标准 Android `Activity` 提供的 `SurfaceView` 上。Rust 通过 JNI 将
`Surface` 转为 `ANativeWindow`，再使用 wgpu/Vulkan 绘制，不需要 X11、Wayland、
Termux:X11、proot 或 Linux 桌面环境。

### 1.1 主要目标

- 使用 Android 原生窗口、生命周期、Insets、剪贴板和系统输入法；
- 尽量复用同一固定 WezTerm revision 的终端语义与远程协议实现；
- 提供普通 SSH 与持久化 SSHMUX 两种远程客户端模式；
- 支持硬件键盘、固定底部应用键盘、中文等系统 IME 文本输入；
- 提供标签切换、新建、关闭、安全分离、动态标题和自动重附着；
- 在移动端提供适合 TUI 的触摸滚动、历史浏览与文本选择；
- 保持终端模型、网络会话、Android Surface 三者生命周期相互独立；
- 默认纯黑终端背景，使用 Material 3 承载设置与对话框；
- 优先保证 Android 15 ARM64 日用闭环，再逐步扩大设备兼容范围。

### 1.2 明确的非目标

当前阶段不实现：

- 本地 PTY、本地 shell 或本地 mux daemon；
- Unix socket 单实例服务；
- 每个 WezTerm window 对应一个 Android 顶层窗口；
- X11、XCB、Wayland、D-Bus 或桌面兼容层；
- 桌面更新器、串口、桌面通知与完整桌面配置兼容；
- 对桌面 `wezterm-gui` 所有功能的一比一复刻。

## 2. 当前能力与证据边界

| 能力 | 当前状态 | 边界 |
|---|---|---|
| Android 原生 Surface + wgpu/Vulkan | 已完成 | Android 15 / Mali-G615 MC6 真机通过 |
| WezTerm terminal cell 模型 | 已完成 | ANSI、TrueColor、宽字符、组合字符与 scrollback 有测试 |
| 字体与 glyph atlas | 已完成核心闭环 | Meslo Nerd Font + Noto CJK；复杂 cluster、彩色 emoji 待完善 |
| 普通 SSH | 已完成 | host-key、密码/交互认证、私有密钥、PTY 与 resize |
| SSHMUX | 已完成核心闭环 | attach、输入、标签控制、安全分离、自动重附着 |
| 动态标签标题 | 已完成 | 随远端 pane/OSC 标题快照更新，变化检测后通知 UI |
| 移动输入 | 已完成核心闭环 | 固定键盘 + 独立系统 IME 编辑框整串发送 |
| 触摸与剪贴板 | 已完成核心闭环 | 单指远端滚轮、双指本地历史、长按选择、复制粘贴 |
| Material 3 设置 | 已完成 | 动态色、深色模式、语言和开发者信息 |
| 中英文界面 | 已完成 | English、简体中文、跟随系统 |
| 后台恢复 | 已完成工程闭环 | 进程存活保持；失效连接或冷启动自动重附着 |
| 16 KB 页兼容 | 仅静态验证 | ELF/APK 对齐通过，尚无 16 KB 页真机运行证据 |
| 正式发布签名 | 未完成 | `v0.1.0` GitHub Release 仍使用 debug 签名 |

“已完成”只表示表中限定的功能和验证环境通过，不等价于完整桌面 WezTerm、所有
Android 设备或长期网络压力测试已经完成。

## 3. 总体结构

```text
MainActivity（唯一终端窗口）             SettingsActivity
  ├─ 顶部状态 / SSH / MUX / 设置             ├─ Material 3
  ├─ MUX 标签控制                            ├─ 每应用语言
  ├─ TerminalSurfaceView                     └─ 开发者与版本信息
  ├─ TerminalKeyboardView
  ├─ WindowInsets / Clipboard / ActionMode
  └─ NativeBridge JNI
                   │
                   ▼
wezterm-android-native（Rust cdylib）
  ├─ ANativeWindow 与 wgpu Surface 所有权
  ├─ Vulkan renderer / glyph atlas 上传
  ├─ TerminalSnapshot 与选择状态
  ├─ SSH / SSHMUX 会话互斥和 JNI 边界
  └─ Surface、terminal、transport 生命周期协调
                   │
       ┌───────────┼───────────┬──────────────┐
       ▼           ▼           ▼              ▼
android-core   android-font  android-ssh   android-mux
  │               │           │              │
wezterm-term   FreeType     wezterm-ssh   wezterm-client
termwiz/cell   HarfBuzz     libssh        mux / codec
```

Android 只有一个终端顶层窗口。设置页是辅助 Activity，不对应远程 WezTerm window，
也不持有终端模型或网络会话。

## 4. 模块职责

| 模块 | 职责 | 不负责 |
|---|---|---|
| `app` | Activity、SurfaceView、键盘、手势、剪贴板、对话框、语言和设置 | 解析 ANSI、持有远端协议状态 |
| `wezterm-android-native` | JNI、wgpu renderer、Surface 所有权、跨模块协调 | Android 业务页面 |
| `wezterm-android-core` | `wezterm-term` 输入、cell/style/scrollback 快照、键序列 | GPU 与网络 |
| `wezterm-android-font` | Meslo/CJK fallback、HarfBuzz shaping、FreeType raster、atlas | 完整桌面 fontconfig |
| `wezterm-android-ssh` | 普通 SSH、host-key、认证、PTY、resize 与事件 | 本地 PTY、SSH agent 自动扫描 |
| `wezterm-android-mux` | `ClientDomain`、远端 pane、标签、codec、detach/reconnect seam | 本地 mux server |
| `vendor/wezterm-ssh-android` | Android 所需的隔离版 SSH 依赖 | 面向 crates.io 发布 |
| `vendor/dirs-next-android` | 把上游 HOME/XDG 查询映射到应用私有目录 | 修改系统或进程全局 HOME |

所有 WezTerm Git 依赖固定到同一个完整 revision，避免 terminal、mux、client 和 codec
之间出现协议或类型漂移。

## 5. 核心数据流

### 5.1 原生窗口与渲染

```text
SurfaceView.surfaceCreated
  → JNI Surface
  → ANativeWindow_fromSurface
  → AndroidNdkWindowHandle
  → wgpu Vulkan Surface
  → TerminalSnapshot
  → shaped glyph / R8 alpha atlas
  → present
```

`TerminalSnapshot` 是渲染边界。网络和终端模型只产生不可变 cell 快照，GPU 代码不
直接持有 WezTerm screen 锁。每个 cell 保存 grapheme、列位置、宽度、前景/背景色、
强度、下划线、斜体、删除线和不可见属性。

终端默认背景由 terminal palette 和 renderer 同时固定为纯黑色。没有连接时，
`idle_cat_ansi` 通过同一个 WezTerm terminal/字体/renderer 通路显示居中的像素猫，
不是独立 Android 图片覆盖层。

### 5.2 字体

```text
cell grapheme
  → MesloLGS Nerd Font Mono
  → 无字形时选择 Noto Sans CJK SC
  → HarfBuzz shape
  → FreeType alpha bitmap
  → 1024 × 1024 atlas
  → wgpu shader
```

默认字体为随 APK 打包的 MesloLGS Nerd Font Mono Regular，包含常用 Nerd/Powerline
私有区字形。CJK 当前从 Android 系统 Noto CJK 字体加载。JetBrains Mono 资产仍保留
用于回归和 fallback seam 测试，但不是默认终端 face。

### 5.3 普通 SSH

```text
连接对话框
  → app-private known_hosts / identity
  → wezterm-ssh + libssh
  → xterm-256color PTY
  → bytes → wezterm-term → TerminalSnapshot
```

普通 SSH 适合无需远端安装 WezTerm 的连接，但 Android 进程或网络连接终止后无法
恢复同一个 shell；持久工作优先使用 SSHMUX。

### 5.4 SSHMUX 与动态标签

```text
Android endpoint
  → ClientDomain(Ssh)
  → remote wezterm cli proxy
  → codec 45
  → ClientPane / remote tabs
  → MuxViewSnapshot { terminal, tabs }
```

每次活动 pane 快照都会驱动 `ClientRenderable` 的自适应远端 poll。快照同时携带标签
ID、pane ID、标题和活动状态；`wezterm-android-mux` 将其与上一次已发送值比较，只有
内容变化时才发出 `TabsChanged`。因此 OSC/pane 标题能够动态更新，而 100 ms UI poll
不会重复发送相同列表。

安全分离和关闭标签页严格区分：

- **Detach** 只断开 Android 镜像，保留远程标签和进程；
- **Close tab** 会终止所选远程标签内的 pane，必须二次确认。

### 5.5 输入

- 物理按键经 Android key code、Unicode code point 和 modifier 转为终端键序列；
- 固定底部应用键盘提供英文、常用符号、Esc、Tab、Ctrl、Alt、编辑键、方向键和
  F1–F12，并占用布局高度而不是覆盖终端；
- 中文等复杂文本在独立输入框中使用系统 IME 完成 composing，点击发送后作为完整
  UTF-8 字符串写入终端；
- 输入会退出历史浏览并回到实时底部，避免用户在旧画面中看不到回显。

### 5.6 触摸、历史和选择

- 单指上下拖动/fling：向远端 pane 发送滚轮事件，让 TUI 自己处理；
- 双指上下拖动/fling：改变客户端 `viewport_offset`，浏览 WezTerm scrollback；
- 长按：按词进入 cell 选择模式；拖动扩展选区；
- Android `ActionMode`：复制、粘贴、选择当前可见屏和取消；
- 双宽字符 continuation cell 会回映射到原 grapheme，软换行不会插入额外换行。

### 5.7 设置与本地化

`SettingsActivity` 使用同一 Material 3 DayNight 主题和系统动态颜色。语言通过
`AppCompatDelegate` 与 Android 每应用语言配置切换，可选跟随系统、English 和
简体中文。设置页还显示应用版本、开发者和公开源码链接。

## 6. 生命周期模型

三个核心状态必须分离：

| 状态 | 创建 | 销毁 | 是否应影响其他状态 |
|---|---|---|---|
| Android Surface / renderer | `surfaceCreated` | `surfaceDestroyed` | 不结束 terminal 或 transport |
| terminal model / viewport | 首次启动或远端快照 | 进程结束或显式重置 | 不拥有 ANativeWindow |
| SSH/SSHMUX transport | 用户连接或自动重附着 | 显式断开、失败清理、进程结束 | Surface 消失时继续存活 |

`onStop` 只暂停 UI poll 和未执行的重试定时器，不主动 Detach。回到前台时：

1. 重建 `ANativeWindow`、wgpu Surface 和 glyph atlas；
2. 从仍存活的 SSH/SSHMUX 会话恢复 terminal 与标签状态；
3. 若 session handle 存在但 `ClientDomain` 已失效，首次快照失败只上报一次；
4. UI 在线程外安全拆除旧 runtime，并按 1/2/4/8/15/30 秒退避重附着；
5. 用户显式 Detach 会关闭自动重附着，不被恢复逻辑抵消。

创建新 Surface 前必须先释放仍残留的 renderer/producer；远端 resize 失败只能记为
transport 警告，不能把一个已成功创建的 GPU Surface 标记为失败。

## 7. 安全与数据边界

- `known_hosts`、导入密钥和连接端点位于应用私有目录或偏好中；
- host key 改变是阻断错误，不自动覆盖；
- 密码和 keyboard-interactive 响应不写入偏好或日志；
- 输入日志只允许记录长度，不记录用户输入内容；
- 默认禁用系统 SSH agent 和任意用户目录 identity 扫描；
- debug 免密脚本只向 debug 应用私有目录 provision 独立 Ed25519 私钥；
- 私钥、SDK 路径和构建机状态不得进入 Git 或 Release；
- `v0.1.0` APK 为 debug 签名版本，不作为正式密钥分发方案；
- 后续正式版本应使用 Android Keystore、文件选择器和独立 release signing。

## 8. 平台基线

| 项目 | 当前值 |
|---|---|
| `minSdk` | 24 |
| `targetSdk` / `compileSdk` | 37 |
| NDK | 28.2.13676358 |
| 发布 ABI | `arm64-v8a` |
| 已验证设备 | OPD2407 / OP615AL1 |
| 已验证系统 | Android 15 / API 35 |
| 已验证 GPU | Mali-G615 MC6 / Vulkan |
| 已验证方向 | 横屏 2800 × 2000 |

ELF `LOAD` 段和 APK native entry 已通过 16 KB 静态对齐检查，但测试设备运行时页大小
为 4 KB；在 16 KB 页设备或模拟器上运行成功之前，不声明 16 KB 运行验证完成。

## 9. 后续路线

### 优先级 A：发布与安全

- 正式 release signing、版本升级策略和可复现发布流程；
- Android Keystore 与密钥导入/口令 UI；
- 隐私审计、依赖许可总览和崩溃处理；
- 16 KB 页 Android 设备或模拟器运行验证。

### 优先级 B：连接韧性

- Wi-Fi/蜂窝切换、Doze、长时间后台压力测试；
- 可选前台服务及其明确的耗电/通知语义；
- TLS domain；
- SSHMUX 首次 host-key 与交互式认证直接桥接。

### 优先级 C：终端体验

- 横竖屏、分屏、折叠屏和系统 IME Insets 压力回归；
- URL/OSC 8 链接识别与打开；
- Android 风格选择手柄和边缘自动滚动；
- 彩色 emoji、复杂 Indic/ZWJ cluster、多 glyph cell；
- 粗体/斜体 face、通用 OEM 字体发现和持久 glyph cache；
- pane 分割、选择和更多 MUX 操作。

具体实现历程、问题根因和回归证据统一记录在
[开发与维护记录](MAINTENANCE.md)。
