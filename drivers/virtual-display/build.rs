use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=wdk/abi.h");
    for name in [
        "XSCC_DRIVER_WDK_INCLUDE",
        "XSCC_DRIVER_SDK_INCLUDE",
        "XSCC_DRIVER_CRT_INCLUDE",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let include = |name| {
        PathBuf::from(env::var_os(name).unwrap_or_else(|| {
            panic!("{name} is required to read the Windows Driver Kit; use the driver build script")
        }))
    };
    let wdk = include("XSCC_DRIVER_WDK_INCLUDE");
    let sdk = include("XSCC_DRIVER_SDK_INCLUDE");
    let crt = include("XSCC_DRIVER_CRT_INCLUDE");
    let mut bindings = bindgen::Builder::default()
        .header("wdk/abi.h")
        .clang_args([
            "-x", "c++", "-std=c++20", "--target=x86_64-pc-windows-msvc",
            "-D_WIN64", "-D_AMD64_", "-DAMD64", "-DUMDF_DRIVER",
            "-DUMDF_VERSION_MAJOR=2", "-DUMDF_VERSION_MINOR=25",
            "-DUMDF_MINIMUM_VERSION_REQUIRED=25",
            "-DIDDCX_VERSION_MAJOR=1", "-DIDDCX_VERSION_MINOR=10",
            "-DIDDCX_MINIMUM_VERSION_REQUIRED=3",
            "-DUNICODE", "-D_UNICODE", "-D_WIN32_WINNT=0x0A00",
        ])
        .allowlist_type("(_?WDF_(DRIVER_CONFIG|OBJECT_ATTRIBUTES|OBJECT_CONTEXT_TYPE_INFO|PNPPOWER_EVENT_CALLBACKS))")
        .allowlist_type("(_?WDFFUNCENUM)")
        .allowlist_type("(_?IDD(FUNCENUM|STRUCTENUM))")
        .allowlist_type("(PFN_IDD_CX|IDD_CX_CLIENT_CONFIG)")
        .allowlist_type("PFN_(WDF(DRIVERCREATE|DEVICEINITSETPNPPOWEREVENTCALLBACKS|DEVICECREATE|OBJECTGETTYPEDCONTEXTWORKER|OBJECTDELETE)|IDDCX(DEVICEINITCONFIG|DEVICEINITIALIZE|ADAPTERINITASYNC|ADAPTERSETRENDERADAPTER|MONITORCREATE|MONITORARRIVAL|SWAPCHAINSETDEVICE|SWAPCHAINRELEASEANDACQUIREBUFFER2?|SWAPCHAINFINISHEDPROCESSINGFRAME|SWAPCHAINREPORTFRAMESTATISTICS))")
        .allowlist_var("(WdfFunctions_02025|WdfDriverGlobals|IddDriverGlobals|IddClientVersionHigherThanFramework|IddFunctionCount|IddStructureCount|IddStructures|IDD_ALWAYS_AVAILABLE_FUNCTION_COUNT)")
        .allowlist_type("(IDXGI(Factory1|Factory4|Adapter1|Device|Resource|Surface)|D3DKMT_(OPENADAPTERFROMLUID|QUERYADAPTERINFO|CLOSEADAPTER|ADAPTERTYPE))")
        .allowlist_type("D3D11_CREATE_DEVICE_FLAG")
        .allowlist_function("(CoCreateGuid|CompareStringOrdinal|CreateDXGIFactory1|D3D11CreateDevice|D3DKMT(OpenAdapterFromLuid|QueryAdapterInfo|CloseAdapter)|QueryPerformanceCounter|RegGetValueW)")
        .allowlist_var("(IID_IDXGI(Factory1|Factory4|Adapter1|Device)|D3D11_SDK_VERSION|D3D11_CREATE_DEVICE_BGRA_SUPPORT|RRF_RT_REG_SZ|ERROR_(FILE_NOT_FOUND|PATH_NOT_FOUND)|XSCC_ABANDON_SWAPCHAIN|CSTR_EQUAL)")
        .generate_comments(false)
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(true);
    for dir in [
        wdk.join("wdf/umdf/2.25"),
        wdk.join("10.0.26100.0/um/iddcx/1.10"),
        crt,
        sdk.join("um"),
        sdk.join("shared"),
        sdk.join("ucrt"),
        sdk.join("winrt"),
    ] {
        bindings = bindings
            .clang_arg("-isystem")
            .clang_arg(dir.to_string_lossy());
    }

    bindings
        .generate()
        .expect("generating pinned WDK bindings")
        .write_to_file(
            PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory")).join("wdk.rs"),
        )
        .expect("writing pinned WDK bindings");
}
