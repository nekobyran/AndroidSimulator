# Android Simulator

Android Simulator 是 Windows 优先的 Android 应用桌面化运行时。它用自有 QEMU/WHPX
后台运行第三方 BlissOS，并把每个 Android 应用通过 scrcpy 4.1 虚拟显示呈现为独立
Windows 窗口；不会显示、嵌入或伪装 Android 整机窗口。

## 已落地体验

- WinUI 3 启动器：Desktop Acrylic → Mica → 纯色回退，Material 橙强调色，分组列表而非逐项卡片。
- 后台模拟器：QEMU 使用 `CREATE_NO_WINDOW`、WHPX 和无窗口 `egl-headless`；运行资源由 `settings.json` 控制，
  默认均衡档为 4 vCPU / 6 GB，节能档为 2 vCPU / 3 GB，性能档为 6 vCPU / 8 GB，自定义支持 2–12 核 / 3–16 GB。
  Windows 调度核心会按档位设置 QEMU/scrcpy 优先级，并在空闲时降到节能调度。3 GiB 仍是 BlissOS 16 保持非 low-RAM 的下限。
- 按需启动：打开启动器和刷新首页只读取轻量 `owned status`，不会启动 QEMU；进入应用库、打开快捷方式
  或双击 APK 时才启动/复用后台 Android，避免空闲占用和前台卡顿。
- APK 双击：当前用户 `.apk` 关联到 `AndroidSimulator.App.exe --apk "%1" --background`。
- 双击安装完成后：创建 package 专属虚拟显示、打开独立 scrcpy 窗口并自动创建桌面快捷方式。
- Android 应用窗口：可信 scrcpy 客户区嵌入 WinUI Desktop Acrylic 宿主，真实应用图标位于左上，
  右上提供左右旋转、置顶和全屏；视频边缘到边缘显示，不再绘制整圈橙色描边。
- 自适应顶栏：仅在嵌入、激活、缩放或旋转事件后，通过受信任子 HWND 的 DWM/SDL 捕获采样 Android
  画面上边缘，生成可读的顶栏染色与深浅前景；采样会合并且不持续轮询，Android 视频像素保持真实、
  不伪造透明度。
- 多窗口：每个 package 对应受信任的 scrcpy PID、独立 `display_id` 和原子元数据；多个应用共享
  一台 Android 实例，不按应用复制虚拟机。
- 键位：只向可信前台 scrcpy PID 对应的 `display_id` 注入；元数据不可信时放行主机按键，
  不回退 display 0。

## 第三方 OS 与供应链

- OS：BlissOS Generic FOSS 16.9.7，Android 13 / API 33，安全补丁 `2024-05-05`。
- 官方项目：<https://sourceforge.net/projects/blissos-x86/>
- ISO：`Bliss-v16.9.7-x86_64-OFFICIAL-foss-20241011.iso`
- SHA-256：`735cb962ec6bd92b62eb82a812831a38d79a0dfdf12b7973d2d0f7ab001ba68e`
- scrcpy：Genymobile 官方 4.1，资产 SHA-256
  `5b12172b3264b2889f4583ee64752ce832e29bc8b1089dca81093459697165db`。

Provision 会先验证 ISO，再提取 kernel/initrd、生成可追溯的 Android Simulator initrd，并写入
`runtime-image.json`。下载文件与 image 目录通过同卷硬链接共享物理数据，不重复占用约 2 GB。

## 架构

- `src/AndroidSimulator.App`：.NET 10 / WinUI 3 启动器、应用库、APK 入口、窗口材质和键位作用域。
- `src/AndroidSimulator.App.Tests`：APK/快捷启动参数的无 UI 合约测试。
- `rust/simulatorctl`：BlissOS/QEMU 生命周期、ADB 就绪、APK、scrcpy 虚拟显示、快捷方式和输入控制平面。
- `android-agent`：可选 guest 扩展点；当前控制通道是仅回环暴露的 owned ADB。
- `RUNTIME.md`：镜像、后台运行、持久化、网络和无 fallback 契约。
- `design-system/android-simulator/MASTER.md`：WinUI 源码级视觉与交互契约。

## D 盘运行与构建

所有 SDK、运行时和缓存位于 `D:\vibecoding\sdk`：

- QEMU：`D:\vibecoding\sdk\msys64\ucrt64`
- Android SDK/ADB：`D:\vibecoding\sdk\android`
- scrcpy：`D:\vibecoding\sdk\scrcpy`
- 产品运行时：`D:\vibecoding\sdk\android-simulator-runtime`
- 构建/临时缓存：`D:\vibecoding\sdk\cache`

常用入口（PowerShell 7）：

```powershell
command\android-simulator.cmd -Action ProvisionOwnedRuntime
command\android-simulator.cmd -Action LaunchOwnedRuntime
command\android-simulator.cmd -Action OwnedStatus
command\android-simulator.cmd -Action ListApps
command\android-simulator.cmd -Action InstallApk -Apk 'D:\packages\reader.apk'
command\android-simulator.cmd -Action LaunchApp -Package com.example.reader -AppName Reader
command\android-simulator-build.cmd
command\android-simulator-doctor.cmd
command\android-simulator.cmd -Action TestWindows
command\android-simulator.cmd -Action RegisterApkAssociation
command\android-simulator.cmd -Action ApkAssociationStatus
command\android-simulator.cmd -Action Check
```

## 数据与性能边界

- 8 GB sparse raw ext4 数据盘持久保存 Android 应用和设置，`snapshot=false`。
- QEMU 使用 virtio storage/network/rng、writeback、threaded AIO、discard 和 zero detection。
- 主显示使用无窗口 `virtio-vga-gl,xres=640,yres=480 + egl-headless`，以最小加速扫描面维持 Android
  系统兼容而不为永不展示的整机桌面浪费合成内存；每应用虚拟显示仍按宿主 DPI 独立创建；
  initrd 同时修复 BlissOS x86_64 Codec2 的 ARM64 linker 搜索顺序，独立应用画面由 scrcpy 稳定编码。
- 启动应用前同时验证 API 与 Android `supports-multiwindow`；low-RAM 会被明确阻断，不再产生空白窗口。
- 启动器状态查询不连接或引导 ADB、不创建 QEMU；`app list --start` 只在明确进入应用库/命令操作时使用。
- Windows 文件锁错误 32/33 都按同一启动实例的正常竞争等待，不再把并发快捷方式/APK 请求误报为启动失败；
  WinUI 增量读取控制进程输出并在其退出后有界关闭管道，不等待已分离的长期 scrcpy 子进程。
- 未配置有效键位时不安装全局键盘 hook；每个可见应用只保留自己的 WinUI 宿主与 scrcpy 子窗口，
  关闭宿主只结束该 package 的 scrcpy，不影响后台 Android 或其他应用窗口。
- 只接受 WHPX，不回退慢速 TCG、官方 AVD、SDK `emulator.exe`、可见 QEMU 窗口或共享整机显示。
- scrcpy 4.1 使用独立虚拟显示与 `--flex-display`：窗口拖动/缩放时 Android VirtualDisplay 直接跟随 SDL 窗口，不再走旧的 180 ms ADB `wm size/density` 去抖链。捕获源上限为 60 FPS；开启帧插值时，增强 scrcpy 会保留前后真实解码帧并按时间间隔生成中间帧，呈现目标可到 240 FPS。
  超分辨率会同时放大 VirtualDisplay 像素与 density，保持 dp 布局不变，再由渲染器高质量缩回宿主窗口；倍率 100–200%。画质策略对应 4/8/16 Mbit/s，渲染器可选 Direct3D/OpenGL，VSync 为真实 SDL renderer 开关。独立应用窗口仍关闭音频，主 Android 显示可单独启用音频，避免多窗口重复声道。
- 控制层在 QEMU/scrcpy 启动和复用时应用持久化性能策略：均衡档 Normal、性能档 AboveNormal、空闲 QEMU BelowNormal + EcoQoS；前台 WinUI 宿主只对路径校验通过的进程刷新调度状态。发布版优先使用同目录 `simulatorctl.exe`，不会误用工作区陈旧 Debug 控制层。
- Release 只放产品外壳和 `simulatorctl.exe`，不重复捆绑 QEMU、ADB、scrcpy、.NET 或 WebView。

## 已验证运行链

真实 BlissOS 启动后 `sys.boot_completed=1`、API `33`、Bliss `16.9.7`，ADB 经
`127.0.0.1:15555` online；真实 APK 已完成安装并由 scrcpy 4.1 创建 `display_id=1` 的
独立应用窗口，并持续存活通过编码验证。低于 API 30 的镜像仍会明确 blocked，绝不回退整机窗口。
