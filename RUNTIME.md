# Android Simulator Runtime

## 不变量

1. 产品后端是 owned QEMU/WHPX + BlissOS，不是 Android Studio AVD 或官方 Emulator。
2. QEMU 无窗口后台运行；用户只看到 package 专属 scrcpy Windows 窗口。
3. 每个活动窗口必须有受信任 scrcpy PID、package 和服务端确认的 `display_id`。
4. 多个应用窗口共享同一 Android 数据环境；禁止按应用复制 VM 或回退 display 0。
5. Android API 必须不低于 30；ADB 非 `device`、WHPX/工具缺失或元数据不可信时明确 blocked。

## Owned backend

- backend: `owned-qemu-blissos`
- instance: `android-simulator`
- image: `blissos-16.9.7-x86_64-foss`
- OS: BlissOS Generic FOSS 16.9.7 / Android 13 / API 33
- security patch: `2024-05-05`
- official source: <https://sourceforge.net/projects/blissos-x86/files/Official/BlissOS16/FOSS/Generic/>
- ISO SHA-256: `735cb962ec6bd92b62eb82a812831a38d79a0dfdf12b7973d2d0f7ab001ba68e`
- ADB endpoint: `127.0.0.1:15555`
- QEMU: `D:\vibecoding\sdk\msys64\ucrt64\bin\qemu-system-x86_64.exe` 11.0.2
- scrcpy: `D:\vibecoding\sdk\scrcpy\scrcpy.exe` 4.1
- runtime root: `D:\vibecoding\sdk\android-simulator-runtime`

`runtime-image.json` 保存来源 URL、固定 SHA-256、OS/API/安全补丁、kernel/initrd 路径和变更说明。
Provision 把已验证下载通过硬链接安装到 image 目录，并从 ISO 提取启动文件；没有未校验镜像 fallback。

## 启动契约

- `-accel whpx`，拒绝 TCG。资源由 `runtime_root/settings.json` 控制：节能档 2 vCPU / 3072 MB、均衡档 4 vCPU / 6144 MB、性能档 6 vCPU / 8192 MB，自定义为 2–12 vCPU / 3072–16384 MB。低于 3 GiB 不受支持，因为 BlissOS 16 会进入 low-RAM 并关闭 multi-window。
- `-device virtio-vga-gl,xres=640,yres=480 -display egl-headless,gl=on -serial none`；用最小主扫描面提供 virgl/Android SystemUI 兼容但不创建 QEMU UI 或控制台窗口。用户应用仍使用独立 DPI-scaled 虚拟显示。
- `CREATE_NO_WINDOW` 启动，按 instance 加锁并保存已验证 QEMU PID；调度策略随性能模式应用：均衡 Normal、性能 AboveNormal；空闲 QEMU 降为 BelowNormal + EcoQoS。可信复用时会再次校验精确可执行路径并刷新调度状态。
- Windows `LockFileEx` 的 sharing/lock violation（原始错误 32/33）均表示同一启动锁正在竞争，必须有界等待；
  并发快捷方式或 APK 激活不得立即报错。
- 启动器首页只调用 `owned status`（PID/精确可执行文件校验），不会连接 ADB 或隐式启动 VM；
  `app list --start`、应用启动和 APK 安装才会按需启动并等待 Android ready。
- 8 GB sparse raw ext4 数据盘，`snapshot=false`，应用与设置跨启动持久保存。
- virtio storage/network/rng，writeback + threaded AIO + discard/zero detection。
- guest `adbd` 监听 5555；QEMU 只把它转发到宿主 `127.0.0.1:15555`。
- initrd 在 boot completed 时为 `wifi_eth` 配置 QEMU usernet IPv4 和 Android 主路由规则，
  然后启动 ADB；宿主不会把 15555 暴露到局域网。
- initrd 在 APEX linker 配置生成后移除 sphal 中错误的 ARM/ARM64 system 库优先项，并启用 Gralloc 4；
  这避免 x86_64 Codec2 误载 ARM64 `libbase.so` 后崩溃。真正的用户画面由按需 scrcpy 虚拟显示输出。

## 每应用窗口

启动参数：

```text
scrcpy --serial 127.0.0.1:15555 \
  --new-display=<settings-derived-size>/<density> --flex-display \
  --no-window-aspect-ratio-lock --no-vd-system-decorations \
  --render-driver=<direct3d|opengl> \
  --max-fps=<capture-fps<=60> --max-size=<quality/supersampling ceiling> \
  --video-bit-rate=<4M|8M|16M> --video-codec=h264 --video-buffer=0
```

初始 VirtualDisplay 由分辨率模式与 Windows DPI 生成；自适应、720p、1080p、1440p 和自定义模式都通过同一 settings 模型持久化。开启超分时像素尺寸与 density 同比例放大（100–200%），因此 Android 的 dp 布局保持不变。
窗口创建后由 scrcpy `--flex-display` 持续把 VirtualDisplay 尺寸跟随 SDL 窗口，WinUI 只负责立即铺满受信任子 HWND；不再通过延迟 ADB `wm size/density` 参与每次 resize。应用在 scrcpy 确认虚拟显示 id 后由显式 `am start --display <id>` 启动，避免显示注册竞态。

Android 捕获源限制为 30–60 FPS。未开启插帧时呈现帧率等于捕获帧率；开启插帧时增强 scrcpy 保留连续解码帧并按源时间戳生成中间帧，呈现目标允许 30–240 FPS。VSync、renderer 和画质码率均来自 settings；复用窗口必须与当前完整命令契约一致，否则只对路径再次验证过的旧 scrcpy 做受控替换。

元数据位于：

```text
D:\vibecoding\sdk\android-simulator-runtime\app-windows\<window_pid>.json
```

记录包含 package、标题、真实图标缓存路径、scrcpy PID、Android API、`display_id`、工具精确路径、
完整命令、日志和状态。标题不是身份凭证；WinUI 必须验证中央 scrcpy 路径、活跃 PID/HWND 与元数据
后才可嵌入 Acrylic 宿主或接收键位。宿主不绘制整圈橙色边框，只在嵌入/激活/缩放/旋转事件后
通过 `PrintWindow` 捕获受信任 SDL/GPU 子窗口的顶部窄条并一次性染色标题栏；不持续轮询画面，
也不从桌面 `GetPixel` 误读独立合成表面后方的窗口。

## APK 与快捷方式

WinUI 启动时按当前 EXE 路径修复 HKCU `.apk` 关联：

```text
"AndroidSimulator.App.exe" --apk "%1" --background
```

同时注册当前用户 `AndroidSimulator.ApkIcon.dll`（`IPersistFile` + `IExtractIconW`）作为 `.apk`
ProgID 的 per-file IconHandler。资源管理器为每个 APK 提取真实应用图标到
`%runtime%/app-icons/shell/<sha16>.ico`；解析失败时回退 `DefaultIcon`（主程序图标），不崩溃
Explorer。`simulatorctl apk shell-icon --apk <path>` 可单独预热/校验该缓存。

双击流程：校验路径 → 确保后台 Android/ADB ready → 安装 APK → 解析 package 与真实应用标签 → 创建虚拟显示 →
获取可信窗口元数据 → 创建桌面 `.lnk` → 前置 scrcpy 窗口。任何一步失败都显示真实错误，不伪造成功。

启动时先检查当前 EXE 的关联状态，只有缺失或路径漂移时才重写 HKCU/通知 Explorer；没有有效键位
映射时不安装全局键盘 hook，也不保留隐藏 WinUI 宿主进程。

## 验证门禁

完成前至少验证：

- QEMU 进程命令含 WHPX、`-display egl-headless,gl=on` 且无可见整机窗口。
- `adb -s 127.0.0.1:15555 get-state` 返回 `device`。
- `sys.boot_completed=1`、`ro.build.version.sdk=33`、`ro.bliss.version=16.9.7`。
- APK install 返回 Success，scrcpy 元数据状态 `ready` 且 `display_id` 非空。
- `ro.config.low_ram=false` 且 `cmd activity supports-multiwindow` 返回 `true`。
- `.apk` HKCU 关联和桌面快捷方式目标/参数精确匹配当前 WinUI EXE。
- Rust fmt/test/clippy、WinUI Debug build 和可见窗口截图验证通过。
