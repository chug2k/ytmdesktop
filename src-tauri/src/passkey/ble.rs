//! Rust side of `macos_bluetooth.m`: a scan that delivers caBLE adverts and
//! Bluetooth state changes over a channel.

use std::{
    ffi::c_void,
    sync::mpsc::{self, Receiver, SyncSender},
};

unsafe extern "C" {
    fn ytm_ble_scan_start(
        ctx: *mut c_void,
        on_advert: extern "C" fn(*mut c_void, *const u8, usize),
        on_state: extern "C" fn(*mut c_void, i32),
    ) -> *mut c_void;
    fn ytm_ble_scan_stop(handle: *mut c_void);
}

pub enum BleEvent {
    Advert(Vec<u8>),
    State(BleState),
}

/// `CBManagerState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleState {
    Unsupported,
    Unauthorized,
    PoweredOff,
    PoweredOn,
    Other,
}

impl From<i32> for BleState {
    fn from(state: i32) -> Self {
        match state {
            2 => BleState::Unsupported,
            3 => BleState::Unauthorized,
            4 => BleState::PoweredOff,
            5 => BleState::PoweredOn,
            _ => BleState::Other,
        }
    }
}

/// Adverts repeat many times a second; drop them rather than queue without
/// bound while the worker is busy.
const QUEUE: usize = 64;

extern "C" fn on_advert(ctx: *mut c_void, data: *const u8, len: usize) {
    // SAFETY: `ctx` is the sender `Scanner` owns, alive until
    // `ytm_ble_scan_stop` returns, and `data` is valid for `len` bytes for
    // the duration of the call.
    let (tx, bytes) = unsafe {
        (
            &*(ctx as *const SyncSender<BleEvent>),
            std::slice::from_raw_parts(data, len),
        )
    };
    let _ = tx.try_send(BleEvent::Advert(bytes.to_vec()));
}

extern "C" fn on_state(ctx: *mut c_void, state: i32) {
    // SAFETY: as in `on_advert`.
    let tx = unsafe { &*(ctx as *const SyncSender<BleEvent>) };
    let _ = tx.try_send(BleEvent::State(state.into()));
}

/// Scanning stops when this is dropped.
pub struct Scanner {
    handle: *mut c_void,
    ctx: *mut SyncSender<BleEvent>,
}

// SAFETY: the handle is only passed back to `ytm_ble_scan_stop`, which is
// thread-safe (it synchronises on the scanner's own queue).
unsafe impl Send for Scanner {}

impl Scanner {
    pub fn start() -> (Self, Receiver<BleEvent>) {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let ctx = Box::into_raw(Box::new(tx));
        // SAFETY: `ctx` stays valid until `Drop`, after the scan has stopped.
        let handle = unsafe { ytm_ble_scan_start(ctx.cast(), on_advert, on_state) };
        (Self { handle, ctx }, rx)
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        // SAFETY: `ytm_ble_scan_stop` returns only after the last callback,
        // so nothing reads `ctx` once it is freed.
        unsafe {
            ytm_ble_scan_stop(self.handle);
            drop(Box::from_raw(self.ctx));
        }
    }
}
