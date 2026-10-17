XSCC GPUI Linux foreground preview

Extract the entire archive, then run ./launch.sh from a logged-in Linux x86_64
Wayland desktop. Do not run as root. This archive does not install a system
service, modify input-device permissions, or configure unattended access.

The host and controller are separate builds. The host must stay open for the
foreground session. Approve screen-sharing and remote-control requests in your
desktop portal. Cancelling a portal request must leave capture/control denied.

Runtime prerequisites include a compatible Wayland compositor, the session D-Bus,
xdg-desktop-portal with your compositor's RemoteDesktop/ScreenCast backend,
PipeWire plus its GStreamer plugin, a PulseAudio-compatible audio server (including
pipewire-pulse), GTK3/GLib, libpulse, libxkbcommon, Fontconfig, and the dynamically
linked libraries required by this build. This is not a self-contained AppImage.
Use ldd ./xscc-host or ldd ./xscc-controller to identify missing shared libraries.

The user-session host uses the desktop portal. Do not use --server/--service with
this package: they do not provide a supported Linux GPUI unattended setup.
No physical-device, compositor, audio or hardware-codec acceptance is implied by
a successful build. Hardware codec availability is selected by runtime probing.

This preview uses CPU-backed BGRA frame presentation; GPU zero-copy is not claimed.
Test display switching, keyboard layout, pointer alignment, focus-loss key release,
audio playback/capture, permission cancellation and reconnect on actual devices.
