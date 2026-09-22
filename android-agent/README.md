# Android Simulator Agent

Stage 1 keeps the Android-side service as a skeleton. The Windows shell and Rust
control plane can install and launch Animeko without this service.

The agent is reserved for later system-level control:

- foreground app and activity state
- frame/performance counters from inside Android
- root/remount capability probes
- host-to-guest command acknowledgement
- controlled file bridge and diagnostics
