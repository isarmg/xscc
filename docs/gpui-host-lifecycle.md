# Windows GPUI host lifecycle

The Windows host is a separately compiled application (`native-host`), installed as
`Program Files\XSCC-Host\xscc-host.exe`. The controller is built with `live-session`.
Renaming a controller executable does not turn it into a host.

## Service and configuration

- `xscc-host.exe --install-service-only`: administrator-only, creates and starts the
  automatic LocalSystem service `XSCC Host`. An existing service must refer to the
  same installed executable and `--service` command. A conflict is an error, never
  silently replaced. The command returns nonzero on failure.
- `xscc-host.exe --service`: Windows service dispatcher. Its existing native
  session supervisor launches the same executable with `--server` as SYSTEM in
  the active Windows session. That process runs actual desktop capture, encoded
  transport, input, and the single management owner.
- `xscc-host.exe`: GPUI configuration, elevated by its administrator manifest.
  It does not start a user-owned fallback manager or copy credentials into the
  interactive user's profile. If the service is absent/stopped, the UI reports it.
- `xscc-host.exe --uninstall-service-only`: stops and deletes only a matching
  service, returning nonzero on failure. It preserves management identity and
  configuration. The installer must not delete application files when this fails.

Management uses the existing protected main IPC. Only this host build permits a
SYSTEM owner and an elevated administrator client, with the same executable
verified on both ends. Other IPC message authorization and controller same-user
verification are unchanged. The host must run from its Program Files installation.
Both service commands and the configuration window reject an untrusted location.

The service owner stores its installation UUID, binding and credentials in the
system account's Known Folder LocalAppData `xscc-managed` directory, with existing
private DACL enforcement. Uninstall keeps this directory; reinstall therefore
keeps identity. There is no second user-profile host identity.

In the configuration window, enter the management HTTPS address and the host
instance's “密码” (server authorization code). Configure up to four reachable
IP/domain + port entries; leave them empty to report addresses from the default
network interface. The direct listener defaults to TCP 21118. Router/NAT and
firewall reachability are deployment requirements, and the application neither
installs a driver nor configures a TLS reverse proxy.

The host refuses connections until bound. A valid current server-issued permit
is mandatory; after scope/privacy checks, managed login goes directly into the
native session without legacy password or approval-window fallback. Revocation
continues through the existing permit checks. The controller never starts an
incoming direct listener. The legacy application updater is disabled for this
host build; updates use the dedicated host installer.

## Verification boundary

The Linux development environment cannot verify Windows SCM, UAC, capture/input,
reboot recovery, or secure-desktop behavior. A successful Windows compile is not
physical-device streaming acceptance. Windows tests cover rejected unknown peer
PIDs; existing IPC tests cover executable mismatches. Installed service/UI tests
must verify a non-administrator client is rejected, a different executable is
rejected, service restarts preserve UUID, and an unbound/revoked host rejects both
legacy and managed session attempts.
