# utensil-ds

Android device-idle state machine daemon. Watches `debug.tracing.screen_state` and drives light/deep idle transitions via direct binder calls to `IPowerManager` and `IBatteryStats`.

## State Machine

```
                    ┌─────────────────────────────┐
                    │         PROP WAIT            │
                    │  block on screen_state prop  │
                    └────────────┬────────────────┘
                                 │
               ┌─────────────────┴──────────────────┐
               │ screen=0 (OFF)      screen=1 (ON)   │
               ▼                         ▲           │
    ┌──────────────────┐                 │ cancel    │
    │  LIGHT IDLE WAIT │─────────────────┘           │
    │  timerfd 90s     │  (eventfd cancel via epoll) │
    └────────┬─────────┘                             │
             │ 90s elapsed                           │
             ▼                                       │
    ┌──────────────────┐                             │
    │ isInteractive()  │── still on ────────────────▶│
    │  power binder    │                             │
    └────────┬─────────┘                             │
             │ still off                             │
             ▼                                       │
    ┌──────────────────┐                             │
    │ noteDeviceIdle() │  IBatteryStats              │
    │  LIGHT (mode=1)  │                             │
    └────────┬─────────┘                             │
             │                                       │
    ┌────────▼─────────┐                             │
    │  DEEP IDLE WAIT  │─────────────────────────────┘
    │  timerfd 360s    │  (eventfd cancel via epoll)
    └────────┬─────────┘
             │ 360s elapsed
             ▼
    ┌──────────────────┐
    │ noteDeviceIdle() │  IBatteryStats
    │  DEEP (mode=2)   │
    └────────┬─────────┘
             │
             └──────────────────▶ PROP WAIT
```

### Timer cancellation

Each screen-off event spawns a thread running the idle sequence with its own `Reactor` (epoll), a `timerfd` for the delay, and a shared `Arc<Fd>` eventfd as the cancel signal. The thread blocks in `epoll_wait` with no wakeups during the wait. Screen-on writes `1` to the eventfd; the idle thread wakes immediately and exits. Zero polling — cancellation latency is kernel scheduler latency only.

### Binder transaction codes

Resolved at runtime from `/system/framework/framework.jar` via `coreshift_core::dex::find_transaction_code`:

| Service | Descriptor | Field |
|---|---|---|
| `power` | `Landroid/os/IPowerManager$Stub;` | `TRANSACTION_isInteractive` |
| `batterystats` | `Landroid/os/IBatteryStats$Stub;` | `TRANSACTION_noteDeviceIdleMode` |

No hardcoded numeric codes — version-agnostic across Android 10–15+.

## Build

### Prerequisites

- Rust stable (1.70+)
- Android NDK r25+ with `aarch64-linux-android` target
- `cargo-ndk` (optional but recommended)

### Add target

```sh
rustup target add aarch64-linux-android
```

### With cargo-ndk

```sh
cargo ndk -t arm64-v8a -p 31 build --release
```

### Manual NDK

```sh
export NDK=$HOME/android-ndk-r25c
export AR=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-ar
export LINKER=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android31-clang

CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$LINKER \
cargo build --release --target aarch64-linux-android
```

Binary at `target/aarch64-linux-android/release/utensil-ds`.

### Push and run

```sh
adb push target/aarch64-linux-android/release/utensil-ds /data/local/tmp/
adb shell chmod +x /data/local/tmp/utensil-ds
adb shell /data/local/tmp/utensil-ds
```

## SELinux permissions

The daemon requires the following SELinux rules (add to your device policy):

```
# Read/wait on debug.tracing.screen_state
allow utensil_ds_exec property_socket:sock_file write;
allow utensil_ds_exec property_service:property_service set;

# Read framework.jar for DEX tx-code resolution
allow utensil_ds_exec system_file:file read;
allow utensil_ds_exec system_file:file open;

# Binder calls to power and batterystats services
allow utensil_ds_exec power_service:service_manager find;
allow utensil_ds_exec batterystats_service:service_manager find;
allow utensil_ds_exec power_service:binder call;
allow utensil_ds_exec batterystats_service:binder call;

# Load libbinder_ndk.so
allow utensil_ds_exec system_lib_file:file { read open execute };
allow utensil_ds_exec system_lib_file:file map;

# Property watching
allow utensil_ds_exec debug_prop:file read;
allow utensil_ds_exec debug_prop:file open;
```

Type `utensil_ds_exec` should be defined in `file_contexts` pointing at the installed binary path (e.g. `/system/bin/utensil-ds`).

## License

MPL-2.0
