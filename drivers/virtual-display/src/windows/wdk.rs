use std::{mem::size_of, ptr};

#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    unused_imports,
    clippy::all
)]
pub(super) mod abi {
    include!(concat!(env!("OUT_DIR"), "/wdk.rs"));
}

unsafe extern "C" {
    #[link_name = "IddFunctions"]
    static IDD_FUNCTIONS: [abi::PFN_IDD_CX; abi::IddFunctionTableNumEntries as usize];
}

pub(super) const fn size<T>() -> u32 {
    size_of::<T>() as u32
}

// The WDK stub populates these tables before DriverEntry. Slot numbers and signatures
// come from the same pinned headers as the stubs linked into the driver.
pub(super) unsafe fn wdf_slot(index: abi::WDFFUNCENUM) -> abi::WDFFUNC {
    unsafe { *abi::WdfFunctions_02025.add(index as usize) }
}

pub(super) fn idd_available(index: abi::IDDFUNCENUM) -> bool {
    (index as u32) < abi::IDD_ALWAYS_AVAILABLE_FUNCTION_COUNT
        || unsafe {
            abi::IddClientVersionHigherThanFramework == 0 || (index as u32) < abi::IddFunctionCount
        }
}

pub(super) unsafe fn idd_slot(index: abi::IDDFUNCENUM) -> abi::PFN_IDD_CX {
    let table = ptr::addr_of!(IDD_FUNCTIONS).cast::<abi::PFN_IDD_CX>();
    unsafe { *table.add(index as usize) }
}

pub(super) fn client_config_size() -> u32 {
    if unsafe { abi::IddClientVersionHigherThanFramework == 0 } {
        return size::<abi::IDD_CX_CLIENT_CONFIG>();
    }
    let index = abi::INDEX_IDD_CX_CLIENT_CONFIG as usize;
    if index < unsafe { abi::IddStructureCount as usize } {
        unsafe { *abi::IddStructures.add(index) as u32 }
    } else {
        0
    }
}

macro_rules! wdf {
    ($slot:ident, $signature:ident, $($arg:expr),* $(,)?) => {{
        let function = std::mem::transmute::<abi::WDFFUNC, abi::$signature>(wdk::wdf_slot(abi::$slot));
        function.expect("invariant: required UMDF function bound by the WDK")(abi::WdfDriverGlobals, $($arg),*)
    }};
}

macro_rules! idd {
    ($slot:ident, $signature:ident, $($arg:expr),* $(,)?) => {{
        let function = std::mem::transmute::<abi::PFN_IDD_CX, abi::$signature>(wdk::idd_slot(abi::$slot));
        function.expect("invariant: required IddCx function bound by the WDK")(abi::IddDriverGlobals, $($arg),*)
    }};
}

pub(super) use {idd, wdf};
