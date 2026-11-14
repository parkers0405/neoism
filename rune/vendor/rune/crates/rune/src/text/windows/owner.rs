//! A COM object that owns a font's bytes, so DirectWrite reads them in place: an in-memory
//! font file reference holds it, and the last release drops the bytes.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use parley::fontique::Blob;
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface};

#[repr(C)]
struct Owner {
    vtable: *const IUnknown_Vtbl,
    refs: AtomicU32,
    _blob: Blob<u8>,
}

static VTABLE: IUnknown_Vtbl = IUnknown_Vtbl { QueryInterface: query_interface, AddRef: add_ref, Release: release };

/// An `IUnknown` holding `blob` until DirectWrite and Rune both let it go.
pub(super) fn new(blob: Blob<u8>) -> IUnknown {
    let owner = Box::new(Owner { vtable: &VTABLE, refs: AtomicU32::new(1), _blob: blob });
    // SAFETY: `Owner` starts with an IUnknown vtable and its one reference moves to the result.
    unsafe { IUnknown::from_raw(Box::into_raw(owner).cast()) }
}

unsafe extern "system" fn query_interface(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    // SAFETY: COM passes valid pointers; only IUnknown itself is offered.
    unsafe {
        if out.is_null() || iid.is_null() {
            return windows::Win32::Foundation::E_POINTER;
        }
        if *iid == IUnknown::IID {
            add_ref(this);
            *out = this;
            return HRESULT(0);
        }
        *out = std::ptr::null_mut();
        windows::Win32::Foundation::E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    // SAFETY: `this` is a live `Owner` (COM's contract).
    unsafe { (*this.cast::<Owner>()).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    // SAFETY: `this` is a live `Owner`; the last release frees the box `new` made.
    unsafe {
        let left = (*this.cast::<Owner>()).refs.fetch_sub(1, Ordering::Release) - 1;
        if left == 0 {
            std::sync::atomic::fence(Ordering::Acquire);
            drop(Box::from_raw(this.cast::<Owner>()));
        }
        left
    }
}
