XSCC One-KVM native Windows Web host

Extract the whole archive to a persistent directory. The executable embeds the
Rust/WASM frontend; no GPUI, Node.js, browser extension or container is needed.
Run .\Start-OneKvm.ps1, then open http://127.0.0.1:8080/ in a browser.
New installations use %LOCALAPPDATA%\XSCC\One-KVM. If a legacy
one-kvm/one-kvm.db exists next to the executable, startup scripts retain that
data root. Supply -DataDir explicitly when reusing another existing installation.

Stop the host before local instance creation/binding. See the included
managed-kvm-operations.md for --instance and XSCS authorization instructions.
Use a separate reachable native TCP port for each instance.

Optional startup: .\Register-OneKvmTask.ps1 registers an interactive task for the
current user at logon. It does not start immediately, elevate privileges, change
firewall settings, install a Windows service, or modify unrelated tasks.
Use .\Unregister-OneKvmTask.ps1 to remove only this installation's startup task.

Desktop capture and system input require that user to remain logged in to an
interactive desktop. The host does not capture the Windows sign-in screen,
Session 0 or secure desktop, and cannot create independent Windows user seats.
Configured monitor/keyboard/mouse/audio resources belong to each instance;
Windows system input still acts on the shared interactive desktop.

Keep the complete data directory when updating; replacing a binary does not
require unbinding or deleting identities. Use packages built from this XSCC
module, not upstream online updates. LICENSE, THIRD-PARTY-NOTICES.txt, licenses/
and SOURCE.json retain original attribution. Build-info and SHA256 sidecars
identify the package; they are not code signatures or physical-device acceptance.
