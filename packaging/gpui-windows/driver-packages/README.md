# Signed Windows virtual display packages

The Windows host installer uses the original signed AMD64 packages in this
directory. Their source URLs, archive checksums and file inventories are
recorded in the adjacent manifests. The Amyuni archive contains its license
and instructions. The configurable display archive uses the adjacent
`VDD-LICENSE-MIT.txt`.

| Archive | Manifest |
| --- | --- |
| `legacy-virtual-display-amd64.zip` | [Legacy display manifest](../virtual-display-driver.json) |
| `configurable-virtual-display-amd64.zip` | [Configurable display manifest](../virtual-display-vdd-driver.json) |

[The in-tree Rust driver](../../../drivers/virtual-display) can replace the
configurable display package after Windows signing and driver validation.
Update its archive and manifest together, then rebuild the host and installer.
The host compares the installed manifest with the manifest embedded at build
time, so they must describe the same signed package.
