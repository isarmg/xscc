# Native package build scripts

These scripts were imported from Microsoft vcpkg at
`9e593bb18ea69cc5095e012465dcd675a822ed0d`. The build uses local recipes
and does not consult the default vcpkg registry. The vcpkg executable is
a host tool, provisioned separately alongside CMake, Ninja and platform SDKs.
