#[cfg(any(windows, test))]
mod profile;
#[cfg(any(windows, test))]
mod swapchain;
#[cfg(any(windows, test))]
mod timing;
#[cfg(all(windows, not(test)))]
mod windows;
