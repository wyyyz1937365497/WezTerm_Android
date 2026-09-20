# WezTerm Android 开发与维护记录

最后更新：2026-09-14

当前版本：`v0.2.3`

主分支：`main`

WezTerm 上游基线：`d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b`

## 1. 文档维护约定

`docs/` 只维护两份活文档：

- [ARCHITECTURE.md](ARCHITECTURE.md)：记录当前目标、系统边界、模块职责和后续路线；
- 本文：按时间记录开发历程、遇到的问题、根因、解决方法和验证入口。

架构发生变化时直接同步架构文档；完成一个可验证阶段或解决一个值得保留的故障时，
在本文追加记录。历史阶段文档已在 `v0.1.0` 合并进这两份文档，不再继续维护多个 gate
文件。验证默认使用 Git commit/tag、可安装 APK、测试结果、真机日志与截图，不把校验和
作为日常发布门槛。

## 2. 当前维护基线

| 项目 | 当前值 |
|---|---|
| 应用 ID | `com.example.wezterm_android` |
| Android 版本 | `minSdk 24`，`targetSdk 37`，`compileSdk 37` |
| NDK | `28.2.13676358` |
| 首发 ABI | `arm64-v8a` |
| Rust target | `aarch64-linux-android` |
| 默认终端字体 | MesloLGS Nerd Font Mono Regular + Noto Sans Math |
| CJK fallback | Android 系统 Noto Sans CJK SC |
| GPU 路径 | wgpu / Vulkan / `ANativeWindow` |
| 远程模式 | 普通 SSH、SSHMUX |
| 真机基线 | OPD2407 / OP615AL1，Android 15 / API 35，Mali-G615 MC6 |
| 发布性质 | ARM64、release 构建 + debug 密钥签名（保留 debuggable）、GitHub Release |

当前版本已经形成以下实用闭环：原生窗口渲染、WezTerm cell 模型、字体 atlas、普通
SSH、SSHMUX、命名 MUX 连接配置与下拉选择、动态标签标题、安全分离、标签控制与活动
标签恢复、每标签独立滚动位置与历史视口锚定、设置页终端缩放、单排顶部状态栏、应用
内键盘、自动换行且按标签隔离草稿的中文输入框、TUI 滚轮、历史回滚、长按选择、系统
剪贴板、Material 3 设置、中英文界面和失效连接自动重附着。

## 3. 开发历程

### 3.1 P0：原生 Surface 与最小客户端骨架

对应提交：`5bee10f`、`57f9d7d`

- 建立 Kotlin `Activity`、`SurfaceView` 与 Rust `cdylib`；
- JNI 使用 `ANativeWindow_fromSurface` 接管 Surface；
- 通过 `AndroidNdkWindowHandle` 创建 wgpu Vulkan Surface；
- 在 Android 15 ARM64 真机完成网格绘制、present 和前后台 Surface 重建；
- 确认 APK/native 动态依赖不包含 X11、XCB、Wayland 或 D-Bus；
- 固定 NDK、ABI 与 WezTerm 上游 revision。

这一阶段证明“标准 Android 原生窗口 + Rust renderer”路线成立，但只证明窗口与 GPU
闭环，不代表终端、字体或网络已经完成。

### 3.2 P1-A：复用 WezTerm 终端模型

- 引入 `wezterm-term`、`termwiz`、cell/surface 与 escape parser；
- 所有远端字节先进入 `Terminal::advance_bytes`，再生成只读 `TerminalSnapshot`；
- 快照保存 grapheme、cell 宽度、颜色和文本属性；
- 补充 ANSI、TrueColor、中文双宽字符、组合字符、resize 与 scrollback 测试；
- 终端模型和 Android Surface 解耦，Surface 重建不重置 terminal。

这一阶段移除了“直接把字节画成临时点阵”的过渡实现，后续 SSH、SSHMUX 和像素猫都
使用同一终端解析及渲染通路。

### 3.3 P1-B：字体 shaping 与 glyph atlas

- 复用固定 WezTerm revision 使用的 FreeType 与 HarfBuzz；
- 建立 Android 专用字体 seam，绕开桌面 fontconfig、D-Bus 和通知依赖；
- 默认打包 MesloLGS Nerd Font Mono，补齐常用 Nerd/Powerline 私有区字形；
- 从 Android 系统加载 Noto Sans CJK SC fallback；
- 生成 1024 × 1024 R8 alpha glyph atlas 并交给 wgpu shader；
- 删除 P1-A 的临时 `font8x8` 路径。

证据：

- [Android 15 字体与 atlas 截图](../artifacts/p1b-font-atlas/android15-harfbuzz-freetype-atlas.png)
- [Android 15 WezTerm cell 截图](../artifacts/p1a-terminal-core/android15-wezterm-cells.png)

### 3.4 P2：普通 SSH 与移动输入

- 使用 `wezterm-ssh`/libssh 建立真实 `xterm-256color` PTY；
- 支持 host-key 确认、密码和 keyboard-interactive 一次性认证、私有身份文件；
- 远端输出进入 `wezterm-term`，终端 resize 回传到 PTY；
- 实现固定在底部、参与布局测量的英文/符号/特殊键键盘；
- 把 Esc、Tab、Ctrl、Alt、编辑键、方向键和 F1–F12 按使用关系重新分组；
- 中文等复杂文本使用独立系统 IME 输入框，完成 composing 后整串发送；
- 提供仅限 debug 的独立 Ed25519 身份 provision 脚本。

凭据边界：密码不持久化、不写日志；debug 私钥不进入 APK 或 Git；host key 改变时
阻断连接，不自动覆盖。

### 3.5 P3：触摸、历史、选择与剪贴板

对应提交：`0033b32`、`4dd5ef4`

- 单指拖动和 fling 转成远端 `Pane::mouse_event` 滚轮事件，交给 TUI 处理；
- 双指拖动和 fling 修改本地 `viewport_offset`，浏览 WezTerm scrollback；
- 当其他客户端把共享 pane 保持为更高行数时，从物理视口底部截取 Android 可见区；
- 长按按词进入 cell 选择，拖动扩展选区；
- 接入 Android `ActionMode`，支持复制、粘贴、选择可见屏和取消；
- 复制时处理双宽 continuation cell 和软换行语义；
- 输入后自动回到实时底部。

证据：

- [历史回滚](../artifacts/p3-touch-selection/scrollback-history.png)
- [长按选择](../artifacts/p3-touch-selection/long-press-selection-3.png)
- [剪贴板粘贴](../artifacts/p3-touch-selection/clipboard-pasted-2.png)

### 3.6 P4：SSHMUX、标签和连接恢复

对应提交：`4dd5ef4`

- 新增 `wezterm-android-mux`，复用 `wezterm-client`、`mux` 和 codec 45；
- 支持附着远端 mux、读取 pane、切换/新建/关闭标签页；
- 将安全 Detach 与破坏性 Close tab 分开，关闭标签前必须确认；
- 应用私有 `dirs-next` 后端提供上游所需的 HOME/XDG 路径；
- Activity 进入后台不主动 Detach，Surface 重建不销毁 transport；
- 连接已失效但 handle 尚存时，清理旧 runtime 并指数退避自动重附着；
- 明确同步最终 Android Surface 行列，修复异步 pane resize 竞态。

证据：

- [Android 15 SSHMUX、动态标题与当前应用键盘](../artifacts/app-screenshots/v0.1.0-mux-keyboard.png)

### 3.7 移动端体验、设置与动态标签

对应提交：`308aad4`、`377a2e4`、`055b918`

- 无连接时通过 terminal pipeline 显示居中像素猫；
- 终端背景固定为纯黑色，对话框和设置使用 Material 3 DayNight；
- 设置页集中语言、开发者信息、版本和公开源码入口；
- 支持跟随系统、English 与简体中文；
- 合并重复 Detach，仅保留一个明确入口；
- `MuxViewSnapshot` 携带实时 tab/pane 标题，内容变化时才通知 Android UI；
- Activity 因语言切换重建后，从真实 session 恢复 SSH/MUX 状态，不显示陈旧提示。

动态标题已在真机连续截图中验证：远端 pane/OSC 标题改变后，顶部标签无需手动切换便
随快照更新。

### 3.8 数学字符、标签草稿与前后台恢复

发布版本：`v0.1.1`

- 内置 Noto Sans Math，作为 Meslo Nerd Font 与系统 Noto CJK 之间的确定性 fallback，
  补齐 Mathematical Alphanumeric Symbols，避免依赖 OEM 不完整的系统字体；
- 中文 IME 编辑框改为自动换行并最多增长到六行；草稿以稳定远端 tab ID 隔离，每次
  文本变化写入应用私有偏好，并在 `onPause` 同步落盘；重连元数据未就绪前绑定待恢复
  tab ID，且不依据瞬时标签快照删除草稿；
- MUX 快照暴露远端 tab ID，Activity 持久化最后活动 ID，前台恢复和自动重附着时由
  Rust runtime 在首个可见快照前重新聚焦对应标签；
- Launcher 使用 WezTerm 上游 `assets/icon/terminal.png`，外加黑色安全边距，避免
  adaptive icon mask 裁掉原图边缘。

### 3.9 终端缩放、标签滚动隔离与单排状态栏

发布版本：`v0.2.0`

- 设置页新增终端缩放滑条（50%–200%，步进 5%）：原生 `cell_size` 按百分比缩放，
  行列数随之增减；缩放变化时按新像素高度重建字体集，字形保持清晰；预览区用白色
  1px 网格勾勒每个字符单元格；
- 返回终端时渲染器就绪回调强制执行一次完整缩放应用：重建字体、重算本地模型行列、
  `resize_remote_pty_if_ready` 同步远端 SSH PTY 与 SSHMUX pane，再重新渲染；
- `TerminalSnapshot` 新增 `viewport_top` 绝对行锚定语义：浏览历史时视口钉在绝对
  物理行，新输出在实时底部追加，不再把正在阅读的内容推出视野；core 与 mux 两侧
  快照统一改为 `snapshot_with_viewport_top`；
- SSHMUX 滚动锚点按远端 tab ID 分别保存在 native `MUX_TAB_VIEWPORTS`，pump 检测
  活动标签变化时保存离场标签锚点并恢复进场标签锚点，覆盖工具栏切换、自动重附
  恢复和远端客户端切换三种路径；激活 JNI 不再清空视口；
- 顶部两排控制按钮合并为一排：MUX 标签四键并入状态栏并按需显示，按钮缩小到
  10–11sp，标签标题由状态栏文本承载。

`v0.1.0` 发布截图：

- [SSHMUX、动态标题与固定键盘](../artifacts/app-screenshots/v0.1.0-mux-keyboard.png)
- [无连接像素猫](../artifacts/app-screenshots/v0.1.0-idle-cat.png)
- [Material 3 设置与版本信息](../artifacts/app-screenshots/v0.1.0-settings.png)

## 4. 关键问题、根因与解决方法

### 4.1 直接交叉编译桌面 GUI 会拉入桌面 Unix 依赖

**现象**：Android 同样满足 Rust 的 `cfg(unix)`，直接使用桌面 `wezterm-gui` 会选择
X11/Wayland 路径，并引入 XCB、XKB、D-Bus、本地 PTY 和 mux server。

**根因**：上游窗口和入口代码的 Unix 条件面向桌面系统，不等价于 Android 平台能力。

**解决**：建立独立 Android 客户端前端和四个职责明确的 Rust seam，只复用 terminal、
font 底层、SSH、client/mux 与 codec，不启动桌面 GUI 和本地服务。

**回归点**：检查 APK/native 动态依赖；构建过程不得需要 X11/Wayland 开发包。

### 4.2 完整 `wezterm-font` 带入 OpenSSL、zbus 和 fontconfig

**现象**：直接依赖完整字体 crate 后，Android 构建进入桌面配置、SSH、通知和系统字体
发现依赖链。

**根因**：桌面字体 crate 不只是 shaping/raster，还集成配置和平台服务。

**解决**：保留固定 revision 的 FreeType/HarfBuzz 版本和语义，建立 Android 专用字体
crate；首版明确使用内嵌 Meslo + Android Noto CJK，而不是移植完整 fontconfig。

**回归点**：字体主机测试、Nerd 私有区 glyph 测试、CJK/组合字符截图和 APK 许可文件。

### 4.3 同进程 Detach 后重新 Attach 卡住

**现象**：首次 SSHMUX 正常，Detach 后同进程再次附着停在 runtime 初始化附近。

**根因**：旧 promise scheduler 的回调/receiver 生命周期未结束；等待旧线程 join 会与新
`set_schedulers` 形成互等，表现为没有网络错误但 attach 永远不完成。

**解决**：Detach domain、shutdown 全局 mux、清空 active session；旧 receiver 继续
drain，直到新 scheduler 安装后自然退出，切换路径不同步 join 旧线程。

**回归点**：同一 Activity 内执行 attach → detach → attach，确认原 pane ID 可恢复且
输入正常。

### 4.4 回前台出现 `native_window_api_connect ... -22`

**现象**：切换应用或重建 Surface 后 renderer 创建失败，终端区域无法恢复。

**根因**：远端 resize 失败导致 JNI 返回 false，但已创建的 native renderer/producer
仍被保留；下一 Surface 尝试连接同一 BufferQueue 时冲突。

**解决**：创建新 Surface 前始终释放旧 renderer；GPU Surface 成功与网络 resize 成功
解耦；`surfaceDestroyed` 无条件且幂等清理；GPU resize 失败也立即 drop renderer。

**回归点**：Android 设置与客户端连续切换，检查每轮 destroy、drop、create、present。

### 4.5 切到其他应用回来仍显示已连接，但终端是空闲猫

**现象**：顶部保留 SSHMUX connected 状态，内容却退回未连接像素猫；必须手动 Detach
再连接。仅熄屏再唤醒不一定触发。

**根因**：应用进入真正后台后，上游 `ClientDomain` 可能已经 detached，但 Java 层
session handle 和 `MUX_READY` 仍是旧值；快照失败路径只显示空闲内容，没有让状态机
退出伪连接状态。

**解决**：首个 snapshot error 只上报一次，同时清除 ready 状态；在线程外安全拆除旧
runtime，按 1/2/4/8/15/30 秒退避自动重附着；`onStop` 只暂停 UI poll/未执行 retry，
不主动 Detach；显式 Detach 清除自动恢复标志。

**回归点**：保持进程，打开另一个应用后返回；分别验证 transport 健康时继续同一 pane，
transport 失效时自动恢复且不出现“已连接 + 猫”的组合状态。

### 4.6 键盘显示/隐藏后终端 Resize 错乱

**现象**：UI 高度已变化，但共享 mux pane 偶尔保留旧行列，提示符位置或历史截取异常。

**根因**：标签更新与异步 mux resize 的完成顺序不稳定，最后生效的不一定是 Android
Surface 的真实尺寸。

**解决**：完成本地 tab 更新后再显式发送最终 Surface 行列；键盘本身参与布局，不覆盖
Surface。

**回归点**：连续显示/隐藏键盘，确认本地和远端在 `101x17` / `101x30` 间同步切换。

### 4.7 单指滚动与 TUI 自有滚动冲突

**现象**：单指直接浏览客户端历史时，`less`、编辑器和其他 alternate-screen TUI 无法
收到自己的滚轮操作。

**解决**：单指统一发送远端滚轮事件；双指才操作客户端历史。状态栏展示离实时底部的
行数，并提供“回到实时”入口。

### 4.8 标签标题只在连接时刷新

**现象**：远端 shell、编辑器或 Codex 更新 pane/OSC title 后，Android 标签仍保留旧名。

**根因**：Rust 快照已经包含 tabs，但 JNI/UI poll 只消费 terminal cell，没有比较和
分发元数据变化。

**解决**：`ClientRenderable` 在每次活动 pane poll 中同时生成 `MuxViewSnapshot`；缓存
最后一次已发送的标签元数据，只在内容改变时发出 `TabsChanged`。

**回归点**：在远端持续改变 pane/OSC title，观察顶部标签自动变化且稳定标题不会触发
重复 UI 更新。

### 4.9 语言切换后状态提示回退

**现象**：设置页切换语言导致 Activity recreation，终端仍连接但状态文案回到初始值。

**根因**：字符串资源正确更新，但 UI 状态只依赖 Activity 内存，没有从 native session
恢复。

**解决**：`onStart` 根据现存 SSH/MUX session 和标签快照重建本地状态文本与控件状态。


### 4.10 SSHMUX 下 Surface 重建后画面被本地占位内容覆盖

**现象**：SSHMUX 附着状态下，冷启动、前后台切换或设置往返后，终端偶尔显示空白或
像素猫占位内容，远端真实画面要等下次输出才恢复。

**根因**：Surface 重建流程会先用本地 `TERMINAL` 模型渲染一帧（MUX 会话下是陈旧
占位），且占位快照的 `viewport_offset` 为 0：`observe_snapshot` 会把历史锚点
`pinned_top` 清空，随后的 mux 快照 diff 判定无变化跳过重绘，占位帧滞留；浏览历史
时还会把该标签已保存的滚动锚点覆盖回实时底部。

**解决**：`nativeSurfaceCreated`、`nativeSurfaceChanged` 与强制缩放应用三条路径在
MUX 就绪时使 `LAST_MUX_SNAPSHOT` 失效并 `pump_mux_terminal()` 重新拉取真实远端
窗格；两条 Surface 回调不再用本地占位快照更新交互状态，滚动锚点由真实 mux 快照
驱动；仅普通 SSH/无连接路径渲染本地快照。

**回归点**：SSHMUX 附着状态下冷启动、前后台切换、缩放调节返回与浏览历史后进设置
再返回，确认画面始终是远端内容、远端行列随缩放同步且滚动位置保持；选择浮动菜单
"保存"导出当前视口顶部经 scrollback 到实时光标行的内容为 `Downloads/` 根目录下
时间戳 txt（SSHMUX 走 `GetLines` RPC，锚点在 UI 线程捕获、RPC 等待在后台线程），
且导出后选择与菜单保持打开。

## 5. 验证方法

### 5.1 Android 构建、JVM 测试和 Lint

```bash
./gradlew :app:testDebugUnitTest :app:lintDebug :app:assembleDebug
```

生成物：

```text
app/build/outputs/apk/debug/app-debug.apk
```

### 5.2 Rust 非 Android UI seam

```bash
cargo test --manifest-path rust/Cargo.toml \
  -p wezterm-android-core \
  -p wezterm-android-font \
  -p wezterm-android-ssh \
  -p wezterm-android-mux \
  --locked -- --test-threads=1
```

当前四个 app crate 共 31 个主机测试：core 13、font 6、SSH 5、MUX 7。

不把以下两类失败误判为 Android 构建失败：

- 全 workspace 主机测试会编译 vendored `wezterm-ssh` 自身测试，其中引用其未声明的
  测试依赖 `k9`；
- Android native crate 在 Linux 主机直接链接测试时找不到 `-landroid`。

Native crate 的有效验证入口是 ARM64 Android 交叉构建和真机加载，不是 Linux host
link。修改 vendor 测试配置或 native seam 后，应单独重新审视这两个边界。

### 5.3 安装与真机检查

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -W -n com.example.wezterm_android/.MainActivity
adb logcat -s WezTermAndroid
```

建议每次影响生命周期、输入或连接的修改至少检查：

1. 冷启动、普通 SSH 与 SSHMUX 各一次；
2. 打开另一个应用后返回，确认同 pane 保持或自动重附着；
3. 显示/隐藏应用键盘和系统 IME，确认 Surface 不被遮挡且 resize 正确；
4. 单指 TUI 滚动、双指历史滚动、长按复制和粘贴；
5. 新建、切换、关闭临时标签和安全 Detach；
6. 远端动态改变标题，确认顶部标签自动刷新；
7. 切换应用语言，确认终端、连接状态和 Material 深色界面保持。

### 5.4 Debug 免密身份

```bash
./scripts/provision-debug-identity.sh USER@HOST [ADB_SERIAL]
```

脚本会为 debug 应用生成独立 Ed25519 key，首次运行可能通过 `ssh-copy-id` 询问一次远端
密码。它只用于开发机和 debug 应用私有目录；不得把生成的私钥复制进源码、APK、文档
或 Release。

## 6. 发布流程

当前版本采用普通 GitHub Release，资产名称明确包含 ABI 和签名性质。维护步骤：

1. 更新 `versionCode` / `versionName`、README、架构与维护文档；
2. 运行第 5 节的 Gradle、Rust 和真机检查；
3. 检查 APK 可安装、版本信息正确、资产大小合理；
4. 提交并推送 `main`；
5. 使用 `gh release create` 创建 tag、正式 Release 和 APK 资产；
6. 用 `gh release view` 检查 tag、target、非草稿状态、非 prerelease 状态和资产下载地址；
7. 确认工作树干净且本地 `main` 与 `origin/main` 一致。

在 release signing、Android Keystore、更多 ABI 和长期网络回归完成前，不把 debug APK
描述为生产稳定版。

## 7. 已知限制与后续维护重点

### 发布和安全

- 尚无正式 release signing；
- SSH 密钥导入、口令保护与 Android Keystore 尚未完成；
- SSHMUX 首次 host-key/交互认证仍主要依赖预置应用私有身份；
- TLS domain 尚未接入。

### 终端与字体

- 每个 terminal cell 当前只提交首个 shaped glyph；
- 复杂 Indic/ZWJ cluster、彩色 emoji、粗体/斜体 face 尚未完整支持；
- 通用 OEM 字体发现、持久 glyph cache、OSC 8 链接尚未完成；
- Android 原生选择手柄和边缘自动滚动待实现。

### 生命周期与平台

- 当前语义是“进程存活时保持连接，进程被结束后回前台自动重附着”，不是前台服务式
  无限后台保活；
- Wi-Fi/蜂窝切换、Doze 和长时间后台仍需压力测试；
- 横竖屏切换、分屏、折叠屏与各种系统 IME Insets 需要更多设备回归；
- 16 KB 页仅完成 ELF/APK 静态对齐检查，尚无 16 KB 页真机运行证据；
- 首个 Release 仅提供 `arm64-v8a`。

### 功能范围

- 不提供本地 shell/PTTY；
- 尚无 pane 分割与完整桌面 WezTerm 配置兼容；
- 普通 SSH 断线后无法恢复同一个 shell，持久任务应使用 SSHMUX；
- 项目不是 WezTerm 官方 Android 发行版。

任何后续完成项都应同时更新验证方法和已知限制，避免把“代码存在”“测试通过”“单台
真机可用”和“跨设备稳定”混为同一证据等级。
