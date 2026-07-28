//! Windows ACL hardening for secret files (daemon token, pairing tokens,
//! paired-host bearer tokens, device registry, audit log).
//!
//! Unix call sites tighten secrets to `0o600` (files) / `0o700` (dirs); the
//! Windows equivalent is an explicit, *protected* (non-inheriting) DACL
//! granting full control to the file owner and SYSTEM only. Callers must
//! treat any `Err` as "log a warning and continue": filesystems that cannot
//! store ACLs at all (FAT32/ExFAT) would otherwise brick startup.

use std::ffi::c_void;
use std::io;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, GENERIC_ALL, GENERIC_READ, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
    NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    CreateWellKnownSid, EqualSid, GetAce, WinBuiltinAdministratorsSid, WinLocalSystemSid,
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_MAX_SID_SIZE, WELL_KNOWN_SID_TYPE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_ALL_ACCESS, FILE_READ_DATA};

/// `ACCESS_ALLOWED_ACE_TYPE` from winnt.h — inlined so we don't pull the
/// whole `Win32_System_SystemServices` feature in for one zero constant.
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

/// Frees a `LocalAlloc`'d buffer (the security descriptor returned by
/// `GetNamedSecurityInfoW`, the ACL returned by `SetEntriesInAclW`) on drop.
struct LocalGuard(*mut c_void);

impl Drop for LocalGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0) };
        }
    }
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(once(0)).collect()
}

fn win32_err(code: u32) -> io::Error {
    io::Error::from_raw_os_error(code as i32)
}

fn well_known_sid(kind: WELL_KNOWN_SID_TYPE) -> io::Result<[u8; SECURITY_MAX_SID_SIZE as usize]> {
    let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut len = sid.len() as u32;
    let ok = unsafe { CreateWellKnownSid(kind, null_mut(), sid.as_mut_ptr() as PSID, &mut len) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(sid)
}

/// Set an explicit, protected (non-inheriting) DACL on `path` granting full
/// control to the file's owner and SYSTEM only — the Windows counterpart of
/// `chmod 0600` (files) / `0700` (dirs). `PROTECTED_DACL` severs inheritance
/// from the parent directory so broader inherited grants can never reappear
/// on this object.
pub fn harden_owner_only(path: &Path) -> io::Result<()> {
    let wide = wide_path(path);
    unsafe {
        // Fetch the owner SID. It points into the returned descriptor, which
        // therefore has to stay alive until the new DACL has been applied.
        let mut owner: PSID = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let status = GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        );
        if status != ERROR_SUCCESS {
            return Err(win32_err(status));
        }
        let _descriptor = LocalGuard(descriptor);
        if owner.is_null() {
            return Err(io::Error::other("file has no owner SID"));
        }

        let mut system_sid = well_known_sid(WinLocalSystemSid)?;

        let trustee = |sid: PSID| TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: sid as *mut u16,
        };
        let entries = [
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_ALL_ACCESS,
                grfAccessMode: SET_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: trustee(owner),
            },
            EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_ALL_ACCESS,
                grfAccessMode: SET_ACCESS,
                grfInheritance: NO_INHERITANCE,
                Trustee: trustee(system_sid.as_mut_ptr() as PSID),
            },
        ];
        let mut dacl: *mut ACL = null_mut();
        let status =
            SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), null_mut(), &mut dacl);
        if status != ERROR_SUCCESS {
            return Err(win32_err(status));
        }
        let _dacl = LocalGuard(dacl as *mut c_void);

        let status = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            dacl,
            null_mut(),
        );
        if status != ERROR_SUCCESS {
            return Err(win32_err(status));
        }
    }
    Ok(())
}

/// Read the effective DACL on `path` and report whether only the file's
/// owner, SYSTEM, and the Administrators group can read it. Returns
/// `Ok(false)` for a NULL DACL (everyone has full control) or for any
/// access-allowed ACE granting read to another SID. Administrators are
/// tolerated because they can take ownership and read anything regardless;
/// rejecting them would false-positive on default user-profile ACLs.
pub fn check_owner_only(path: &Path) -> io::Result<bool> {
    let wide = wide_path(path);
    unsafe {
        let mut owner: PSID = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let status = GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        );
        if status != ERROR_SUCCESS {
            return Err(win32_err(status));
        }
        let _descriptor = LocalGuard(descriptor);
        // A NULL DACL grants everyone full control.
        if dacl.is_null() {
            return Ok(false);
        }

        let mut system_sid = well_known_sid(WinLocalSystemSid)?;
        let mut admins_sid = well_known_sid(WinBuiltinAdministratorsSid)?;

        // Any of these bits lets the holder read file contents.
        const READ_BITS: u32 = FILE_READ_DATA | GENERIC_READ | GENERIC_ALL;

        for index in 0..u32::from((*dacl).AceCount) {
            let mut ace: *mut c_void = null_mut();
            if GetAce(dacl, index, &mut ace) == 0 {
                return Err(io::Error::last_os_error());
            }
            let header = ace as *const ACE_HEADER;
            if (*header).AceType != ACCESS_ALLOWED_ACE_TYPE {
                continue;
            }
            let allowed = ace as *const ACCESS_ALLOWED_ACE;
            if (*allowed).Mask & READ_BITS == 0 {
                continue;
            }
            let sid = &(*allowed).SidStart as *const u32 as PSID;
            let trusted = EqualSid(sid, owner) != 0
                || EqualSid(sid, system_sid.as_mut_ptr() as PSID) != 0
                || EqualSid(sid, admins_sid.as_mut_ptr() as PSID) != 0;
            if !trusted {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
