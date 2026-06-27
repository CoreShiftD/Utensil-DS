// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/

pub mod prop_wait {
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

pub mod binder_calls {
    use coreshift_core::binder::RawBinderService;
    use coreshift_core::dex::find_transaction_code;
    use coreshift_core::{log_info, log_warn};

    const JAR: &str = "/system/framework/framework.jar";
    const TAG: &str = "utensil-ds:binder";
    pub const IDLE_LIGHT: i32 = 1;
    pub const IDLE_DEEP: i32 = 2;

    pub struct BinderCtx {
        bstats:    RawBinderService,
        idle_code: u32,
    }
    // RawBinderService wraps a *mut c_void dlopen handle. libbinder transactions
    // are thread-safe, and the handle is immutable after construction.
    unsafe impl Sync for BinderCtx {}

    impl BinderCtx {
        pub fn open() -> Result<Self, String> {
            let dc = find_transaction_code(
                JAR,
                "Lcom/android/internal/app/IBatteryStats$Stub;",
                "TRANSACTION_noteDeviceIdleMode",
            )
            .ok_or("dex: TRANSACTION_noteDeviceIdleMode missing")?;

            log_info!(TAG, "tx code: noteDeviceIdleMode={dc}");

            let bstats = RawBinderService::open("batterystats").map_err(|e| e.to_string())?;
            Ok(Self { bstats, idle_code: dc })
        }

        pub fn note_idle(&self, mode: i32) {
            log_info!(TAG, "noteDeviceIdleMode mode={mode}");
            if let Err(e) = self.bstats.transact_i32(self.idle_code, mode) {
                log_warn!(TAG, "noteDeviceIdleMode: {e}");
            }
        }
    }
}

pub mod idle_fsm {
    use crate::binder_calls::{BinderCtx, IDLE_DEEP, IDLE_LIGHT};
    use coreshift_core::reactor::{Event, Fd, Reactor, Token};
    use coreshift_core::{log_error, log_info};
    use std::sync::Arc;
    use std::time::Duration;

    const TAG: &str = "utensil-ds:fsm";

    pub fn make_cancel() -> Arc<Fd> {
        Arc::new(Fd::eventfd(0).expect("eventfd for cancel"))
    }

    fn wait_timer(
        reactor: &mut Reactor,
        timer_tok: Token,
        cancel_tok: Token,
        events: &mut Vec<Event>,
    ) -> bool {
        loop {
            events.clear();
            match reactor.wait(events, 2, -1) {
                Err(_) | Ok(0) => continue,
                Ok(_) => {}
            }
            for ev in events.iter() {
                if ev.token == cancel_tok { return false; }
                if ev.token == timer_tok  { return true; }
            }
        }
    }

    fn drain(fd: &Fd) {
        let mut buf = [0u8; 8];
        while let Ok(Some(_)) = fd.read_slice(&mut buf) {}
    }

    pub fn run(ctx: &BinderCtx, cancel: Arc<Fd>) {
        let mut reactor = match Reactor::new() {
            Ok(r) => r,
            Err(e) => { log_error!(TAG, "Reactor::new: {e}"); return; }
        };
        let timer = match Fd::timerfd() {
            Ok(f) => f,
            Err(e) => { log_error!(TAG, "Fd::timerfd: {e}"); return; }
        };

        let cancel_tok = match reactor.add(&cancel, true, false) {
            Ok(t) => t,
            Err(e) => { log_error!(TAG, "reactor.add cancel: {e}"); return; }
        };
        let timer_tok = match reactor.add(&timer, true, false) {
            Ok(t) => t,
            Err(e) => { log_error!(TAG, "reactor.add timer: {e}"); return; }
        };

        let mut events = Vec::new();

        log_info!(TAG, "screen off — light idle in 90s");
        if let Err(e) = timer.set_timer_oneshot(Some(Duration::from_secs(90))) {
            log_error!(TAG, "arm timer 90s: {e}"); return;
        }
        if !wait_timer(&mut reactor, timer_tok, cancel_tok, &mut events) {
            log_info!(TAG, "cancelled (light wait)"); return;
        }
        drain(&timer);
        ctx.note_idle(IDLE_LIGHT);

        log_info!(TAG, "light idle entered — deep idle in 360s");
        if let Err(e) = timer.set_timer_oneshot(Some(Duration::from_secs(360))) {
            log_error!(TAG, "arm timer 360s: {e}"); return;
        }
        if !wait_timer(&mut reactor, timer_tok, cancel_tok, &mut events) {
            log_info!(TAG, "cancelled (deep wait)"); return;
        }
        drain(&timer);

        ctx.note_idle(IDLE_DEEP);
        log_info!(TAG, "deep idle entered");
    }
}
