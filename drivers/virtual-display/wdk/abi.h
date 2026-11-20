#define NOMINMAX
#define CINTERFACE
#define D3D11_NO_HELPERS
#include <windows.h>
#include <wudfwdm.h>
#include <wdf.h>
#include <iddcx.h>
#include <d3d11.h>
#include <d3dkmthk.h>
#include <dxgi1_5.h>

constexpr NTSTATUS XSCC_ABANDON_SWAPCHAIN = STATUS_GRAPHICS_INDIRECT_DISPLAY_ABANDON_SWAPCHAIN;
