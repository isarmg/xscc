#include "abi.h"

// Keep the WDK's header-defined version records and force the Rust entry point into the DLL.
#pragma comment(linker, "/include:DriverEntry")
