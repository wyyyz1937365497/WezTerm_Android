# P0：Android 原生 Surface → Rust → wgpu/Vulkan

验证时间：2026-09-07（Asia/Shanghai）

## 结论

P0 通过。标准 Android `Activity` 中的 `SurfaceView` 已经能够通过 JNI 把
`Surface` 转为 `ANativeWindow`，再由 Rust/wgpu 在 Vulkan 后端完成原生绘制。
这个闭环完全不经过 X11。

P0 只证明平台窗口和 GPU 通路可用。它不证明 WezTerm 终端模型、字体栅格化、
桌面渲染器、SSH 或 mux 已经移植完成。

## 验证环境

| 项目 | 实测值 |
|---|---|
| 设备 | OPD2407 / OP615AL1 |
| Android | 15 |
| API | 35 |
| ABI | `arm64-v8a` |
| 物理屏幕 | 2000 × 2800，400 dpi |
| 运行时页大小 | 4096 bytes |
| GPU | Mali-G615 MC6 |
| 图形后端 | Vulkan |
| NDK | 28.2.13676358 |
| Rust | 1.98.1 stable |
| cargo-ndk | 4.1.2 |

当前真机是 4 KB 页设备。因此这里对 16 KB 的结论仅为 ELF/APK 静态兼容检查，
不是 16 KB 页真机运行结论。

## Gate 结果

| Gate | 结果 | 依据 |
|---|---|---|
| Rust host check | PASS | `cargo check --workspace --locked` |
| Android ARM64 cross build | PASS | `cargo ndk -t arm64-v8a -P 24 ...` |
| Gradle APK assembly | PASS | `:app:assembleDebug` |
| APK ABI 收敛 | PASS | APK 仅含 `lib/arm64-v8a/libwezterm_android.so` |
| 16 KB ELF 对齐 | PASS（静态） | 四个 ELF `LOAD` 段均为 `Align 0x4000` |
| 16 KB APK 对齐 | PASS（静态） | `zipalign -c -P 16 4` 成功 |
| 原生窗口/Vulkan present | PASS | 真机画面和 renderer ready 日志 |
| X11-free 动态依赖 | PASS | 仅 `libandroid`, `liblog`, `libdl`, `libm`, `libc` |
| IME 桥入口 | PASS（冒烟） | 真机触发 commit 与 composition 回调 |
| Surface 前后台重建 | PASS | 同一 PID 完成 destroy → recreate → ready |
| 中文输入正确性 | NOT TESTED | 留给接入真实终端模型后的输入专项测试 |
| WezTerm 字符/字体渲染 | NOT IMPLEMENTED | P0 使用验证 shader |
| SSH / SSHMUX / TLS | NOT IMPLEMENTED | 后续阶段 |

## 真机关键日志

冷启动：

```text
adapter name=Mali-G615 MC6 backend=Vulkan type=IntegratedGpu
configured native surface 2800x1896 density=400 format=Rgba8UnormSrgb present=Fifo
P0 native Surface renderer is ready
```

前台切到 Android 设置，再返回应用；进程 PID 始终为 `30444`：

```text
released ANativeWindow
destroyed wgpu Surface while retaining the process-level client boundary
adapter name=Mali-G615 MC6 backend=Vulkan type=IntegratedGpu
configured native surface 2800x1896 density=400 format=Rgba8UnormSrgb present=Fifo
P0 native Surface renderer is ready
```

输入桥只记录长度，不记录用户输入内容：

```text
IME commit bytes=1 chars=1
IME commit bytes=1 chars=1
```

## 16 KB 与动态依赖证据

最终 ELF 的 program headers：

```text
LOAD ... R   0x4000
LOAD ... R E 0x4000
LOAD ... RW  0x4000
LOAD ... RW  0x4000
```

最终原生动态依赖：

```text
libandroid.so
liblog.so
libdl.so
libm.so
libc.so
```

不存在 `libX11`、`libxcb`、Wayland 或 D-Bus 依赖。

## P0 产物摘要

下列 APK/`.so` 哈希是 P0 通过时的历史记录；同一路径随后已被 P1 构建覆盖。
长期保留的 P0 证据是截图文件。

| 文件 | SHA-256 |
|---|---|
| `app/build/outputs/apk/debug/app-debug.apk` | `333840834371fd4f8818ddbb6a3b44bb4ee88034fa16223764b29c4e02563211` |
| `app/build/generated/rustJniLibs/p0/arm64-v8a/libwezterm_android.so` | `b7a37e3b420d756d3437e083881e7f9b0ccc0b61be4addb117b4d59af16d28f2` |
| `artifacts/p0-native-surface/android15-native-wgpu.png` | `44eb68bfd33c66b5080fd416ac1fb9a010c4729fb47d7d1b83e43743245410cd` |

APK 是 debug 构建，不是发布签名产物。

## 上游基线审计

P0 开始前审计了 WezTerm 上游提交：

```text
d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b
```

该审计确认现有桌面 GUI 在 Android 上会进入 Unix/X11/Wayland 假设，并且 GUI
入口会建立本地 domain/mux server。后续不会直接把 `wezterm-gui` 编译成 APK，
而会逐步抽取纯客户端所需的共享 crate。
