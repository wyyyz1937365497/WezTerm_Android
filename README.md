# WezTerm Android Native Client

一个实验性的 Android 原生 WezTerm 客户端前端：复用 WezTerm 的终端模型、字体底层、
字符渲染、SSH 与远程 mux 客户端代码，同时使用标准 Android Activity、SurfaceView、
输入法和系统服务。

它不依赖 X11、Wayland、Termux:X11、proot 或 Linux 桌面环境，也不是 WezTerm 官方
Android 发行版。

> 当前版本：`v0.1.0` Developer Preview。仅提供 `arm64-v8a` 调试签名 APK，已在
> Android 15 / API 35 真机完成核心功能验证，不应当作生产稳定版或正式密钥分发渠道。

![Android 15 上的 WezTerm cell、HarfBuzz/FreeType 与 wgpu atlas](artifacts/p1b-font-atlas/android15-harfbuzz-freetype-atlas.png)

## 已实现

- Android `SurfaceView` → JNI `ANativeWindow` → wgpu/Vulkan 原生渲染；
- `wezterm-term` ANSI/TrueColor/cell/scrollback 终端模型；
- MesloLGS Nerd Font Mono + Android Noto Sans CJK fallback；
- 普通 SSH：host-key、认证、`xterm-256color` PTY、输入与 resize；
- SSHMUX：持久远端标签、新建/切换/关闭、安全 Detach 和自动重附着；
- 动态标签标题，跟随远端 pane/OSC title 更新；
- 固定底部英文/符号/特殊键键盘，不覆盖终端 Surface；
- 独立系统 IME 输入框，支持完成中文 composing 后整串发送；
- 单指发送远端滚轮给 TUI，双指浏览本地历史；
- 长按选择、Android 浮动操作栏和系统剪贴板；
- 前后台 Surface 重建、失效连接识别和指数退避恢复；
- 纯黑终端、无连接像素猫、Material 3 深色界面；
- 跟随系统、English、简体中文以及设置页开发者信息。

完整目标和模块边界见 [架构文档](docs/ARCHITECTURE.md)；开发历程、故障根因、验证
方法和已知限制见 [维护文档](docs/MAINTENANCE.md)。`docs/` 只维护这两份活文档。

## 获取 APK

从 [GitHub Releases](https://github.com/wyyyz1937365497/WezTerm_Android/releases)
下载 `wezterm-android-v0.1.0-arm64-debug.apk`。该包为 Developer Preview：

- 仅支持 `arm64-v8a`；
- 使用 Android debug 签名；
- 尚未接入 Android Keystore 和正式 release signing；
- 16 KB 页目前只有静态对齐检查，没有 16 KB 页设备运行证据。

## 本地构建

需要 Android SDK、NDK `28.2.13676358`、JDK、Rust stable、
`aarch64-linux-android` target 和 `cargo-ndk`。

```bash
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked
./gradlew :app:assembleDebug
```

Gradle 会自动构建 ARM64 Rust JNI 库。APK 位于：

```text
app/build/outputs/apk/debug/app-debug.apk
```

安装并启动：

```bash
adb install -r app/build/outputs/apk/debug/app-debug.apk
adb shell am start -W -n com.example.wezterm_android/.MainActivity
```

## Debug 免密身份

需要 SSHMUX 调试时，可以为目标主机创建一把独立的 debug Ed25519 key，并放入应用
私有目录：

```bash
./scripts/provision-debug-identity.sh USER@HOST [ADB_SERIAL]
```

脚本首次运行可能询问一次远端密码。私钥不会进入 APK 或 Git；release 构建不会自动
使用它。不要将生成的身份文件作为正式用户密钥分发。

## 验证

Android 构建、JVM 测试和 Lint：

```bash
./gradlew :app:testDebugUnitTest :app:lintDebug :app:assembleDebug
```

Rust 终端、字体、SSH 与 MUX seam：

```bash
cargo test --manifest-path rust/Cargo.toml \
  -p wezterm-android-core \
  -p wezterm-android-font \
  -p wezterm-android-ssh \
  -p wezterm-android-mux \
  --locked -- --test-threads=1
```

真机原生日志：

```bash
adb logcat -s WezTermAndroid
```

## 当前边界

- 没有本地 PTY、本地 shell、本地 mux server 或桌面窗口兼容层；
- 普通 SSH 断线后不能恢复同一个 shell，持久任务应使用 SSHMUX；
- 复杂 Indic/ZWJ cluster、彩色 emoji、粗体/斜体 face 和链接交互待完善；
- Wi-Fi/蜂窝切换、Doze、长时间后台、横竖屏、分屏和更多 OEM 设备仍需压力回归；
- 当前是“进程存活则保持，连接失效或进程重启后自动重附着”，不是前台服务式无限
  后台保活；
- TLS domain、Android Keystore、密钥导入 UI、多 ABI 和正式签名尚未完成。

## 代码结构

```text
app/
  Android Activity、SurfaceView、键盘、手势、剪贴板、设置与本地化
rust/
  wezterm-android-native   JNI、ANativeWindow、wgpu 与生命周期协调
  wezterm-android-core     wezterm-term 和 TerminalSnapshot
  wezterm-android-font     HarfBuzz、FreeType、Meslo/CJK 与 glyph atlas
  wezterm-android-ssh      普通 SSH 客户端
  wezterm-android-mux      SSHMUX client、标签与恢复
docs/
  ARCHITECTURE.md          当前目标与架构
  MAINTENANCE.md           开发历程、问题与维护方法
```

WezTerm 相关 Git 依赖统一固定在 revision
`d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b`，避免 terminal、mux、client 和 codec
之间产生版本漂移。
