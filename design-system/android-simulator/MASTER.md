# Android Simulator · WinUI design contract

**Platform:** Windows 11 / WinUI 3
**Product:** owned headless Android runtime and multi-window desktop app launcher
**Direction:** compact Fluent utility, Material orange, native acrylic depth, fast and quiet

## Experience contract

- The primary task is launching an Android app, not administering QEMU. QEMU is always background infrastructure.
- A desktop shortcut opens or reuses its package-specific scrcpy window. The launcher may remain hidden while the owned runtime is starting.
- Different packages may remain visible side by side as independent Windows windows; never present or rename a shared QEMU display as an app.
- Every action uses real `simulatorctl` state. No fake apps, metrics, success messages, or decorative controls.
- Chinese is the default product language. Technical identifiers remain selectable and copyable.

## Navigation and hierarchy

Use one WinUI `NavigationView` with four top-level destinations:

1. **概览** — runtime state and the smallest set of useful actions.
2. **应用** — installed launchable packages, APK import, launch, and shortcut actions.
3. **键位** — direct mapping list plus add/edit/delete actions.
4. **设置** — performance profile, material, and runtime facts.

At narrow widths the pane collapses; content remains a single scroll surface. Ordinary app, keymap, and setting rows use `ListView`/`ItemsRepeater`, dividers, hover/selected state layers, and aligned columns. Do not turn every row into a rounded card. A raised surface is reserved for runtime summary, errors, dialogs, and independent detail panels.

## Native component map

| Need | WinUI primitive |
|---|---|
| Top-level navigation | `NavigationView` / `NavigationViewItem` |
| App and keymap collections | `ListView` with semantic item templates |
| Primary action | one `Button` with `AccentButtonStyle` per surface |
| Row actions | `Button`, `HyperlinkButton`, or `MenuFlyout` with `FontIcon` |
| Binary option | `ToggleSwitch` |
| Feedback | `InfoBar`, inline progress, or `TeachingTip` when anchored |
| Confirmation/input | `ContentDialog` with primary/secondary action hierarchy |
| APK choice | `FileOpenPicker` limited to `.apk` |
| Loading | indeterminate `ProgressRing` plus real status text |

Use Fluent system icons only; never use emoji, punctuation, or text characters as icons.

## Material and surface tokens

- Window backdrop: `DesktopAcrylicBackdrop` when supported.
- Fallback order: Desktop Acrylic → Mica → opaque `ApplicationPageBackgroundThemeBrush`.
- Navigation pane: `LayerFillColorDefaultBrush` with system tint; content stays transparent enough for backdrop depth.
- Group surface: `CardBackgroundFillColorDefaultBrush` only for a true grouped summary/detail surface.
- Borders and dividers: WinUI theme resources, one device pixel where needed.
- Accent: Material orange tokens (`#FF9800` light emphasis, `#FFB74D` dark emphasis) override the
  WinUI launcher accent resources consistently; never drift to system blue or decorative gradients.
- Android app hosts do not inherit a decorative orange outline. Their Acrylic title bar samples the
  real Android surface's top edge from the trusted DWM/SDL child after attach, resize, activation, or
  rotation and derives a readable tint/foreground pair. Sampling is coalesced and event-triggered,
  never a continuous pixel polling loop or a desktop-behind-the-window color read.
- Error/warning/success: WinUI semantic brushes and `InfoBarSeverity`, never color alone.
- Light, dark, high-contrast, inactive-window, and transparency-disabled modes must stay readable.

## Typography and density

- Typeface: system `Segoe UI Variable`; no downloaded fonts.
- Page title 28/32 semibold, section title 20/28 semibold, row title 14/20 semibold, body 14/20, caption 12/16.
- Spacing scale: 4, 8, 12, 16, 24, 32. Page gutter 24 at wide widths and 16 when compact.
- Interactive height is at least 32 px for desktop pointer use; primary actions and touch-relevant rows use 40–44 px.
- Package IDs and coordinates may use the system monospace font, but do not reduce readability.

## State matrix

Each data surface must express these states without relying on animation:

| State | Required expression |
|---|---|
| Loading | disabled conflicting actions, progress, concrete status text |
| Ready | current runtime/app/keymap state and available actions |
| Empty | object-specific empty state plus one recovery action |
| Error | `InfoBar`, actionable message, retry when safe |
| Starting | bounded progress and phase text (runtime, Android boot, app launch) |
| Success | updated list/status; transient confirmation may supplement it |
| Disabled | native disabled state plus reason where non-obvious |

## Motion and feedback

- Use native theme transitions for navigation and short opacity/translation changes for loading-to-content and list insertion/removal.
- Target 120–180 ms for hover/selection and 180–220 ms for route/content transitions.
- No looping glow, bouncing, parallax, staged decoration, or layout-shifting hover effects.
- Respect Windows animation/reduced-motion settings. Text, icon, selection, and layout must communicate every state when animation is disabled.

## Desktop ergonomics and accessibility

- Full keyboard traversal, visible focus, Escape to cancel dialogs, Enter only for the current primary action.
- Icon-only actions require `ToolTipService.ToolTip` and accessible names.
- Do not intercept a mapped key unless the foreground HWND belongs to a trusted central scrcpy PID, that PID resolves to a live package-specific `display_id`, and the mapping is enabled.
- Missing or stale window metadata must pass the host key through unchanged; never inject to display 0 as a fallback.
- Preserve selection and scroll position on refresh where practical.
- Launcher minimum: 760 × 540. App-host minimum: 640 × 408 with an initial 16:9 Android viewport.
  Verify at 800 × 600, 1100 × 720, and 1440 × 900 with high DPI.
- Screen-reader names expose app label, package, runtime state, key, action, and enabled state.

## Performance and size rules

- Acrylic is one window-level backdrop; do not stack custom blur layers.
- Avoid large raster hero art, web views, bundled fonts, animation packages, or duplicate icon assets.
- Lists are virtualized and refreshed explicitly; do not poll UI collections continuously.
- Launcher status/refresh is read-only and must not start QEMU. Runtime startup is demand-driven by an explicit start,
  application-library, shortcut or APK action.
- Reuse one `virtio-vga-gl` + `egl-headless` QEMU instance. Create or reuse one scrcpy virtual-display window only for each package the user actually opens.
- Install the global keyboard hook only while at least one valid enabled mapping exists; otherwise allow the direct-launch WinUI process to exit after window handoff.
- QEMU, ADB and scrcpy stay in the central D-drive SDK and are never copied into the application Release.
- Keep process trust, package → PID → `display_id` identity and expensive lifecycle work in the Rust control layer.

## Android application window semantics

- The WinUI launcher/library uses Acrylic. The Android application content remains the real Android surface rendered by scrcpy; do not place a fake Fluent frame inside the video.
- Embed the trusted scrcpy client HWND edge-to-edge below a WinUI Desktop Acrylic title bar. Keep the
  native Windows border/default rounded outer edge; do not draw a full-window orange outline. Adapt the
  title-bar tint to a bounded sample of the real Android top edge while preserving readable controls.
  Never tint, blur or replace the opaque Android video client area.
- The visible window title uses the application label, but title text is presentation only. Foreground, reuse and key mapping trust the registered scrcpy PID/HWND and server-confirmed `display_id`.
- Starting a package has explicit phases: background Android startup, ADB readiness, virtual display creation, application launch and window ready.
- A package window may be starting, ready, disconnected, blocked or closed. A disconnected/offline state must be explained instead of exposing the QEMU screen.
- Android below API 30 is a blocked runtime state because the required scrcpy virtual display cannot be created; never replace it with the main Android display.
- Window close affects that package session only. Other package windows and the background Android runtime remain available.
- Standard scrcpy window resizing, focus, mouse, keyboard and IME behavior should feel like a Windows application while preserving Android aspect ratio and touch semantics.

## Source-level completion checklist

- [ ] Shortcut/package/APK actions reach real `simulatorctl` commands and open/reuse the package-specific scrcpy window.
- [ ] QEMU uses `CREATE_NO_WINDOW` and `egl-headless`; no QEMU title/front/visible-window logic remains.
- [ ] Multiple packages can own separate windows and server-confirmed `display_id` values concurrently.
- [ ] Navigation hierarchy and standard WinUI controls match the component map.
- [ ] App/keymap/settings rows are list semantics, not per-row cards.
- [ ] Loading, empty, error, starting, success, and disabled states exist where applicable.
- [ ] Acrylic has Mica/opaque fallback and high-contrast remains usable.
- [ ] Keyboard focus, tooltips, reduced motion, resize, and DPI behavior are covered.
- [ ] Key mapping resolves trusted foreground scrcpy PID → `display_id`; missing metadata does not swallow the key or target display 0.
- [ ] No fake data, decorative copy, emoji icons, hidden failures, or C-drive build SDK paths remain.
