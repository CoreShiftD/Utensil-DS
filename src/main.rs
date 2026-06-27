// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/

mod prop_wait {
    use coreshift_core::android_property::{
        android_property_find, android_property_read, android_property_serial,
        android_property_wait, AndroidPropertyInfo,
    };

    pub struct ScreenProp {
        info: AndroidPropertyInfo,
        serial: u32,
    }

    impl ScreenProp {
        pub fn open() -> Option<Self> {
            let info = android_property_find("debug.tracing.screen_state")?;
            let serial = android_property_serial(info).ok()?;
            Some(Self { info, serial })
        }

        pub fn wait_change(&mut self) -> Option<String> {
            let s = android_property_wait(self.info, self.serial, None).ok()??;
            self.serial = s;
            android_property_read(self.info).ok().map(|v| v.value)
        }
    }
}

mod binder_calls {
    use coreshift_core::binder::RawBinderService;
    use coreshift_core::dex::find_transaction_code;
    use coreshift_core::{log_error, log_info, log_warn};

    const JAR: &str = "/system/framework/framework.jar";
    const TAG: &str = "utensil-ds:binder";
    pub const IDLE_LIGHT: i32 = 1;
    pub const IDLE_DEEP: i32 = 2;

    pub struct BinderCtx {
        power:            RawBinderService,
        bstats:           RawBinderService,
        interactive_code: u32,
        idle_code:        u32,
    }

    impl BinderCtx {
        pub fn open() -> Result<Self, String> {
            let ic = find_transaction_code(
                JAR,
                "Landroid/os/IPowerManager$Stub;",
                "TRANSACTION_isInteractive",
            )
            .ok_or("dex: TRANSACTION_isInteractive missing")?;

            let dc = find_transaction_code(
                JAR,
                "Landroid/os/IBatteryStats$Stub;",
                "TRANSACTION_noteDeviceIdleMode",
            )
            .ok_or("dex: TRANSACTION_noteDeviceIdleMode missing")?;

            log_info!(TAG, "tx codes: isInteractive={ic} noteDeviceIdleMode={dc}");

            let power  = RawBinderService::open("power").map_err(|e| e.to_string())?;
            let bstats = RawBinderService::open("batterystats").map_err(|e| e.to_string())?;

            Ok(Self { power, bstats, interactive_code: ic, idle_code: dc })
        }

        pub fn is_interactive(&self) -> bool {
            match self.power.transact_bool(self.interactive_code) {
                Ok(v) => v,
                Err(e) => { log_error!(TAG, "isInteractive: {e}"); true }
            }
        }

        pub fn note_idle(&self, mode: i32) {
            log_info!(TAG, "noteDeviceIdleMode mode={mode}");
            if let Err(e) = self.bstats.transact_i32(self.idle_code, mode) {
                log_warn!(TAG, "noteDeviceIdleMode: {e}");
            }
        }
    }
}

mod idle_fsm {
    use crate::binder_calls::{BinderCtx, IDLE_DEEP, IDLE_LIGHT};
    use coreshift_core::log_info;
    use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
    use std::time::Duration;

    const TAG: &str = "utensil-ds:fsm";

    fn sleep_cancellable(secs: u64, cancel: &Arc<AtomicBool>) -> bool {
        for _ in 0..secs {
            if cancel.load(Ordering::Relaxed) { return false; }
            std::thread::sleep(Duration::from_secs(1));
        }
        !cancel.load(Ordering::Relaxed)
    }

    pub fn run(ctx: &BinderCtx, cancel: Arc<AtomicBool>) {
        log_info!(TAG, "screen off — light idle in 90s");
        if !sleep_cancellable(90, &cancel) { log_info!(TAG, "cancelled (light wait)"); return; }

        if ctx.is_interactive() { log_info!(TAG, "screen back on — abort"); return; }
        ctx.note_idle(IDLE_LIGHT);

        log_info!(TAG, "light idle — deep idle in 360s");
        if !sleep_cancellable(360, &cancel) { log_info!(TAG, "cancelled (deep wait)"); return; }

        ctx.note_idle(IDLE_DEEP);
        log_info!(TAG, "deep idle entered");
    }
}

use binder_calls::BinderCtx;
use coreshift_core::{log_error, log_info, log_warn};
use idle_fsm::run as run_idle;
use prop_wait::ScreenProp;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::thread;

const TAG: &str = "utensil-ds";

fn main() {
    log_info!(TAG, "start pid={}", std::process::id());

    let ctx = BinderCtx::open().unwrap_or_else(|e| {
        log_error!(TAG, "binder init: {e}");
        std::process::exit(1);
    });

    let mut prop = ScreenProp::open().unwrap_or_else(|| {
        log_error!(TAG, "property debug.tracing.screen_state not found");
        std::process::exit(1);
    });

    let mut cancel: Option<Arc<AtomicBool>> = None;
    let mut idle_handle: Option<thread::JoinHandle<()>> = None;

    loop {
        let value = match prop.wait_change() {
            Some(v) => v,
            None => { log_warn!(TAG, "wait_change None; retry"); continue; }
        };

        let screen_on = value.trim() != "0";
        log_info!(TAG, "screen_state={value:?} on={screen_on}");

        if let Some(f) = cancel.take() { f.store(true, Ordering::Relaxed); }
        if let Some(h) = idle_handle.take() { let _ = h.join(); }

        if !screen_on {
            let flag = Arc::new(AtomicBool::new(false));
            cancel = Some(flag.clone());
            // Safety: ctx lives for entire process lifetime; thread joins before next iteration.
            let ctx_ptr = &ctx as *const BinderCtx as usize;
            idle_handle = Some(thread::spawn(move || {
                run_idle(unsafe { &*(ctx_ptr as *const BinderCtx) }, flag);
            }));
        }
    }
}
