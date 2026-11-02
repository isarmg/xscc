# XSCC One-KVM bare-metal Web host

`xscc/one-kvm` is maintained inside the XSCC repository with its own Cargo lockfiles and build entry. Deploy native binaries on physical Linux or Windows machines. Both platforms use the Rust/WASM Web interface for configuration and remote access. Runtime containers and upstream online update packages are not supported.

One host can bind multiple independent XSCS `controlled / one_kvm` instances. Each instance owns its controlled identity and credentials, target authorization, hardware configuration and runtime. Select an instance at the host root, then configure its video source/monitor, keyboard and mouse, audio and other available devices at `/instances/<id>/`. Windows desktop capture and system input require the logged-in user's interactive desktop; this does not create independent Windows seats or select physical keyboards through SendInput.

XSCS owns enrollment and controller-to-instance relationships. A browser controller uses its own `controller / one_kvm` code; a native XSCC controller keeps its `controller / rustdesk` identity. Each target requires an explicit access relationship. A protected host-level browser controller pool allows the same controller code to log into multiple authorized targets without replacing its identity; sessions remain target-scoped.

See [operating instructions](docs/managed-kvm-operations.md), [architecture](docs/managed-kvm.md), [import provenance](SOURCE.json), and [upstream introduction](docs/upstream-README.en.md). Preserve the complete data root when upgrading. Original licenses and notices remain in the repository. Software checks and package creation do not establish physical-device acceptance.
