# Local Cargo registry

This registry stores the original crate archives and the index records needed by the repository's Cargo lockfiles. The lockfiles retain the canonical crates.io package identities and SHA-256 checksums. Cargo uses this directory through source replacement in the root `.cargo/config.toml`, including for the separate GPUI and One-KVM workspaces.

After changing a dependency, update the affected lockfiles, archive and index together. Run `python3 .github/scripts/check-local-rust-dependencies.py` from a checkout to verify the stored package versions and checksums. The archives carry the packages' license files and metadata. Locally edited dependencies live under `vendor/platform`, `vendor/gpui` or the relevant feature directory.
