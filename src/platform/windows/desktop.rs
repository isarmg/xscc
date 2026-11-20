use std::{cell::RefCell, io, mem};
use winapi::{
    shared::{minwindef::FALSE, windef::HDESK},
    um::{
        processthreadsapi::GetCurrentThreadId,
        winnt::GENERIC_WRITE,
        winuser::{
            CloseDesktop, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop,
            SetThreadDesktop, DESKTOP_CREATEMENU, DESKTOP_CREATEWINDOW, DESKTOP_ENUMERATE,
            DESKTOP_HOOKCONTROL, DESKTOP_READOBJECTS, DESKTOP_SWITCHDESKTOP, DESKTOP_WRITEOBJECTS,
            UOI_NAME,
        },
    },
};

thread_local! {
    static SWITCHED_DESKTOP: RefCell<Option<SelectedDesktop>> = const { RefCell::new(None) };
}

struct SelectedDesktop {
    original: HDESK,
    current: Desktop,
}

impl Drop for SelectedDesktop {
    fn drop(&mut self) {
        if !self.original.is_null() {
            unsafe { SetThreadDesktop(self.original) };
        }
    }
}

struct Desktop(HDESK);

impl Desktop {
    fn input() -> io::Result<Self> {
        let access = DESKTOP_CREATEMENU
            | DESKTOP_CREATEWINDOW
            | DESKTOP_ENUMERATE
            | DESKTOP_HOOKCONTROL
            | DESKTOP_WRITEOBJECTS
            | DESKTOP_READOBJECTS
            | DESKTOP_SWITCHDESKTOP
            | GENERIC_WRITE;
        let desktop = unsafe { OpenInputDesktop(0, FALSE, access) };
        if desktop.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(desktop))
        }
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe {
            CloseDesktop(self.0);
        }
    }
}

fn name(desktop: HDESK) -> Option<[u16; 256]> {
    if desktop.is_null() {
        return None;
    }
    let mut name = [0u16; 256];
    let mut size = 0;
    (unsafe {
        GetUserObjectInformationW(
            desktop.cast(),
            UOI_NAME as i32,
            name.as_mut_ptr().cast(),
            mem::size_of_val(&name) as u32,
            &mut size,
        )
    } != FALSE)
        .then_some(name)
}

pub(super) fn is_input_selected() -> bool {
    let current = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
    let Ok(input) = Desktop::input() else {
        return false;
    };
    name(current)
        .zip(name(input.0))
        .is_some_and(|(a, b)| a == b)
}

pub(super) fn select_input() -> io::Result<()> {
    let input = Desktop::input()?;
    let original = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
    if unsafe { SetThreadDesktop(input.0) } == FALSE {
        return Err(io::Error::last_os_error());
    }
    SWITCHED_DESKTOP.with(|desktop| {
        let mut desktop = desktop.borrow_mut();
        if let Some(selected) = desktop.as_mut() {
            selected.current = input;
        } else {
            *desktop = Some(SelectedDesktop {
                original,
                current: input,
            });
        }
    });
    Ok(())
}
