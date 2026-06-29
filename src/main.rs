// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/

use utensil_ds::binder_calls::BinderCtx;
use utensil_ds::idle_fsm::{make_cancel, run as run_idle};
use utensil_ds::prop_wait::ScreenProp;
use coreshift_core::reactor::Fd;
use coreshift_core::{log_error, log_info, log_warn};
use std::sync::Arc;
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

    let mut cancel: Option<Arc<Fd>> = None;
    let mut idle_handle: Option<thread::JoinHandle<()>> = None;

    loop {
        let value = match prop.wait_change() {
            Some(v) => v,
            None => { log_warn!(TAG, "wait_change None; retry"); continue; }
        };

        let screen_on = value.trim() == "2";
        log_info!(TAG, "screen_state={value:?} on={screen_on}");

        if let Some(f) = cancel.take() {
            let _ = f.write_u64(1);
        }
        if let Some(h) = idle_handle.take() {
            let _ = h.join();
        }

        if !screen_on {
            let cancel_fd = make_cancel();
            cancel = Some(cancel_fd.clone());
            let ctx_ptr = &ctx as *const BinderCtx as usize;
            idle_handle = Some(thread::spawn(move || {
                run_idle(unsafe { &*(ctx_ptr as *const BinderCtx) }, cancel_fd, None);
            }));
        }
    }
}
