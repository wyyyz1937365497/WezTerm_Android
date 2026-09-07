# P1-B：HarfBuzz / FreeType / wgpu glyph atlas 闭环

验证时间：2026-09-07（Asia/Shanghai）

## 结论

P1-B 以“终端字符显示闭环”的范围通过。Android 15 真机画面不再使用 P1-A 的
`font8x8`：输入字节先经过真实 `wezterm-term`，每个可见 grapheme 再由固定
WezTerm revision 的 HarfBuzz/FreeType 栈处理，FreeType bitmap 被打包到 alpha
atlas 并由 wgpu/Vulkan 采样显示。

真机已显示 JetBrains Mono ASCII、ANSI/TrueColor、下划线、删除线、Noto Sans CJK
中文双宽字符以及组合字符 `e + U+0301`。P1 的停止条件（真实 WezTerm cell、宽字符
与组合字符确定性用例）已经满足。

这不是“完整 `wezterm-font` 已移植”的声明。当前实现是 Android 专用 seam，准确的
未完成边界见本文末尾。

## 上游字体预检与架构决策

固定上游版本仍为：

```text
repository: https://github.com/wezterm/wezterm.git
revision:   d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b
```

对完整上游 crate 执行：

```bash
cargo ndk -t arm64-v8a -P 24 check -p wezterm-font
```

预检首先失败在 `openssl-sys`。依赖树证明 OpenSSL 不是字体栅格化所需，而是经由：

```text
wezterm-font -> config -> wezterm-ssh -> libssh-rs/ssh2/async_ossl -> openssl-sys
```

同时还存在以下桌面耦合：

```text
wezterm-font -> wezterm-toast-notification -> zbus
wezterm-font -> fontconfig   # Android 被现有 Unix cfg 纳入
```

`lfucache` 虽在 `wezterm-font/Cargo.toml` 中直接声明，但该 crate 的源码没有引用它。
因此没有为“编译字体”引入 Android OpenSSL、D-Bus 或 fontconfig，而是建立
`wezterm-android-font`：

- Git 依赖精确 pin 到同一 WezTerm revision 的 `deps/freetype` 与 `deps/harfbuzz`；
- 在 Android seam 内实现生命周期所有权、shaping、alpha rasterization 与 shelf atlas；
- 从同一 revision 引入 `JetBrainsMono-Regular.ttf`，保留 OFL 1.1；
- CJK 暂时从 Android `/system/fonts/NotoSansCJK-Regular.ttc` 的 SC face 加载。

底层隔离探针均通过：

```text
cargo ndk ... check -p freetype  -> PASS
cargo ndk ... check -p harfbuzz  -> PASS
cargo ndk ... check -p wezterm-android-font -> PASS
```

## 实现范围

### 终端快照

`CellSnapshot` 当前保存：

- grapheme 文本、row、column 与 cell width；
- 前景/背景 SRGBA；
- intensity、underline、italic、strikethrough、invisible。

上游默认调色板被收敛为应用深色背景；ANSI、256 色和 TrueColor 仍通过
`ColorPalette::resolve_fg/resolve_bg` 解析。样式化空格会保留，默认空白会省略。

### 字体与 atlas

```text
TerminalSnapshot cell.text
        -> FontSet fallback selection
        -> HarfBuzz shape
        -> FreeType render glyph
        -> deterministic 1024 x 1024 R8 alpha atlas
        -> CellRenderData storage buffer
        -> wgpu fragment shader
```

宽度为 2 的 cell 会把同一 atlas placement 映射到两个物理 cell，并校正第二个 cell
的 glyph origin，避免把 CJK glyph 裁到单列。atlas 在 host 侧有缓存/packing 测试。

## 自动测试

命令：

```bash
cargo test --manifest-path rust/Cargo.toml \
  -p wezterm-android-core -p wezterm-android-font --locked
cargo ndk -t arm64-v8a -P 24 check -p wezterm-android-native --locked
./gradlew :app:assembleDebug --no-daemon
```

结果：

```text
wezterm-android-core: 5 passed, 0 failed
wezterm-android-font: 5 passed, 0 failed
Android ARM64 check: PASS
Gradle assembleDebug: PASS
```

确定性断言包括：

```text
A中B: column=0/1/3, width=1/2/1
e + U+0301: one terminal cell -> one HarfBuzz glyph
ANSI TrueColor: foreground=[1,2,3,255], background=[4,5,6,255]
styled blank retained; default blank omitted
atlas duplicate glyph hits the same placement
```

## Android 15 真机结果

设备与 P0 相同：OPD2407 / OP615AL1、Android 15/API 35、ARM64、400 dpi、
Mali-G615 MC6/Vulkan。

冷启动关键日志：

```text
loaded Android CJK fallback index=1 path=/system/fonts/NotoSansCJK-Regular.ttc face=2
font face index=0 family=JetBrains Mono source=bundled JetBrains Mono Regular pixel_height=38
font face index=1 family=Noto Sans CJK SC source=Noto Sans CJK SC pixel_height=38
adapter name=Mali-G615 MC6 backend=Vulkan type=IntegratedGpu
configured native surface 2800x1896 density=400 format=Rgba8UnormSrgb present=Fifo
prepared glyph atlas glyphs=47 shaped_cells=250 fallback_cells=1 multi_glyph_cells=0
P1-B WezTerm/HarfBuzz/FreeType atlas is ready revision=d2f3f05b... grid=101x36 occupied=250 cursor=0,9
```

切到 Android 设置再返回，PID 始终为 `11689`：

```text
released ANativeWindow
destroyed wgpu Surface while retaining the WezTerm terminal model
loaded Android CJK fallback ...
prepared glyph atlas glyphs=47 ...
P1-B WezTerm/HarfBuzz/FreeType atlas is ready ...
```

这证明 Surface、GPU 和字体 atlas 可以重建，而终端模型仍独立存在。

截图：`artifacts/p1b-font-atlas/android15-harfbuzz-freetype-atlas.png`

## 原生依赖与 16 KB 静态检查

最终 ELF 的四个 `LOAD` 均为 `Align 0x4000`；`zipalign -c -P 16 4` 通过。
APK 只包含 `lib/arm64-v8a/libwezterm_android.so`。

动态 `NEEDED` 保持为：

```text
libandroid.so
liblog.so
libdl.so
libc.so
libm.so
```

HarfBuzz C++ 运行时已静态链接，没有 `libc++_shared.so`；Cargo target 依赖扫描没有
OpenSSL、X11、XCB、Wayland、D-Bus、zbus 或 fontconfig。测试设备仍是 4 KB 页，
所以 16 KB 结论仍只是静态兼容性结论。

## 当前产物

| 文件 | 大小 | SHA-256 |
|---|---:|---|
| `app/build/outputs/apk/debug/app-debug.apk` | 19,057,055 bytes | `08b5e01318296fb42c68c76ed5adf521e323ab17fd922188edee3c136fb6aa28` |
| `app/build/generated/rustJniLibs/p0/arm64-v8a/libwezterm_android.so` | 18,220,904 bytes | `a59c98dc0d2b04aa383a74ed9a80dfa6174ff78964ac525f2de0f8ae8dd14355` |
| `artifacts/p1b-font-atlas/android15-harfbuzz-freetype-atlas.png` | 183,111 bytes | `5152b8318fad0dec5a9dc119f7f8738bd530768ff009e6d3079ba3e4872a2f34` |
| `rust/wezterm-android-font/assets/JetBrainsMono-Regular.ttf` | 273,900 bytes | `a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f` |

APK 是 debug 构建，不是发布签名产物。`generated/rustJniLibs/p0` 是历史命名的
Gradle 中间目录，不表示当前 gate 为 P0。

## 未完成边界

- 尚未直接复用完整 `wezterm-font::FontConfiguration`；当前复用的是同 revision 的
  WezTerm FreeType/HarfBuzz 底层包与字体资产，并由 Android seam 隔离桌面依赖。
- CJK fallback 使用本机已验证的系统路径和 TTC face index；不同 OEM 的通用字体发现
  尚未实现，最终应解析 Android font configuration 或由 Java/Kotlin 字体 API 提供。
- atlas 目前是单通道 alpha；彩色 emoji 未实现，截图中的 emoji 是明确的 tofu。
- 每个 terminal cell 当前只提交 shaped run 的首个 glyph。此次组合字符被合成为单
  glyph，但复杂 Indic、ZWJ emoji 或其他多 glyph cluster 尚不保证正确。
- cell 已携带粗体/斜体属性，但尚未打包并选择 JetBrains Mono Bold/Italic face；粗体
  目前只遵循 WezTerm 的 ANSI brightening 规则。
- atlas 会在 resize/Surface 重建时重建，尚不是桌面 WezTerm 的持久 LRU glyph cache。
- P1 使用固定输入字节流；没有网络、SSH、host-key 或凭据处理。下一 gate 是 P2。
