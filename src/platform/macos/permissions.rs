use cocoa::{
    base::nil,
    foundation::{NSProcessInfo, NSString},
};
use objc::{class, msg_send, rc::autoreleasepool, sel, sel_impl};
use std::{
    ffi::{c_char, c_void},
    io,
    ptr::{null, null_mut, NonNull},
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
    fn IOHIDRequestAccess(request: u32) -> bool;
}

#[repr(C)]
struct AuthorizationItem {
    name: *const c_char,
    value_length: usize,
    value: *mut c_void,
    flags: u32,
}

#[repr(C)]
struct AuthorizationItemSet {
    count: u32,
    items: *mut AuthorizationItem,
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn AuthorizationCreate(
        rights: *const AuthorizationItemSet,
        environment: *const AuthorizationItemSet,
        flags: u32,
        authorization: *mut *mut c_void,
    ) -> i32;
    fn AuthorizationCopyRights(
        authorization: *mut c_void,
        rights: *const AuthorizationItemSet,
        environment: *const AuthorizationItemSet,
        flags: u32,
        authorized_rights: *mut *mut AuthorizationItemSet,
    ) -> i32;
    fn AuthorizationFree(authorization: *mut c_void, flags: u32) -> i32;
}

struct Authorization(NonNull<c_void>);

impl Drop for Authorization {
    fn drop(&mut self) {
        unsafe { AuthorizationFree(self.0.as_ptr(), 0) };
    }
}

pub(super) fn major_version() -> u32 {
    autoreleasepool(|| unsafe {
        NSProcessInfo::processInfo(nil)
            .operatingSystemVersion()
            .majorVersion as u32
    })
}

pub(super) fn screen_recording(prompt: bool) -> bool {
    autoreleasepool(|| unsafe {
        let granted = CGPreflightScreenCaptureAccess();
        if !granted && prompt {
            CGRequestScreenCaptureAccess();
        }
        granted
    })
}

pub(super) fn input_monitoring(prompt: bool) -> bool {
    const LISTEN_EVENT: u32 = 1;
    const GRANTED: u32 = 0;
    const DENIED: u32 = 1;
    const UNKNOWN: u32 = 2;
    match unsafe { IOHIDCheckAccess(LISTEN_EVENT) } {
        GRANTED => true,
        DENIED if prompt => {
            autoreleasepool(|| unsafe {
                let settings = NSString::alloc(nil).init_str(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
                );
                let url: cocoa::base::id = msg_send![class!(NSURL), URLWithString: settings];
                let workspace: cocoa::base::id = msg_send![class!(NSWorkspace), sharedWorkspace];
                let _: cocoa::base::BOOL = msg_send![workspace, openURL: url];
                let _: () = msg_send![settings, release];
            });
            false
        }
        UNKNOWN if prompt => {
            unsafe { IOHIDRequestAccess(LISTEN_EVENT) };
            false
        }
        _ => false,
    }
}

pub(super) fn authorize_admin() -> io::Result<bool> {
    const INTERACTION_ALLOWED: u32 = 1;
    const EXTEND_RIGHTS: u32 = 1 << 1;
    const PREAUTHORIZE: u32 = 1 << 4;
    let mut authorization = null_mut();
    let status = unsafe { AuthorizationCreate(null(), null(), 0, &mut authorization) };
    if status != 0 {
        return Err(io::Error::other(format!(
            "AuthorizationCreate failed ({status})"
        )));
    }
    let authorization = Authorization(NonNull::new(authorization).ok_or_else(|| {
        io::Error::other("AuthorizationCreate returned no authorization reference")
    })?);
    let mut item = AuthorizationItem {
        name: c"system.privilege.admin".as_ptr(),
        value_length: 0,
        value: null_mut(),
        flags: 0,
    };
    let rights = AuthorizationItemSet {
        count: 1,
        items: &mut item,
    };
    let status = unsafe {
        AuthorizationCopyRights(
            authorization.0.as_ptr(),
            &rights,
            null(),
            INTERACTION_ALLOWED | EXTEND_RIGHTS | PREAUTHORIZE,
            null_mut(),
        )
    };
    Ok(status == 0)
}
