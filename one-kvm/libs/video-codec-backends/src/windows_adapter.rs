use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};

#[no_mangle]
extern "C" fn hwcodec_first_nvidia_adapter() -> i32 {
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return -1;
    };
    for index in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        if unsafe { adapter.GetDesc1() }.is_ok_and(|description| description.VendorId == 0x10de) {
            return i32::try_from(index).unwrap_or(-1);
        }
    }
    -1
}
