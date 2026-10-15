#pragma once

#include <windows.h>

#if !defined(_M_X64) && !defined(__x86_64__)
#error RustDesk supports only x86_64 Windows builds.
#endif

inline bool rustdesk_supported_windows()
{
    // Windows 11 retains NT version 10.0; version helpers alone also accept Windows 10.
    const auto ntdll = GetModuleHandleW(L"ntdll.dll");
    const auto kernel32 = GetModuleHandleW(L"kernel32.dll");
    if (!ntdll || !kernel32)
        return false;
    using GetVersion = LONG(WINAPI*)(OSVERSIONINFOEXW*);
    using GetMachine = BOOL(WINAPI*)(HANDLE, USHORT*, USHORT*);
    const auto get_version = reinterpret_cast<GetVersion>(GetProcAddress(ntdll, "RtlGetVersion"));
    const auto get_machine = reinterpret_cast<GetMachine>(GetProcAddress(kernel32, "IsWow64Process2"));
    OSVERSIONINFOEXW version = {};
    version.dwOSVersionInfoSize = sizeof(version);
    USHORT process_machine = 0, native_machine = 0;
    return get_version && get_machine && get_version(&version) == 0 &&
           version.dwMajorVersion == 10 && version.dwMinorVersion == 0 &&
           version.dwBuildNumber >= 22000 && version.wProductType == VER_NT_WORKSTATION &&
           get_machine(GetCurrentProcess(), &process_machine, &native_machine) &&
           native_machine == IMAGE_FILE_MACHINE_AMD64;
}
