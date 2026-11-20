vcpkg_extract_source_archive(SOURCE_PATH
    ARCHIVE "${CURRENT_PORT_DIR}/amf-interface.tar.gz"
)

# Install the AMF headers to the default vcpkg location
file(INSTALL "${SOURCE_PATH}/AMF/" DESTINATION "${CURRENT_PACKAGES_DIR}/include/AMF")
vcpkg_install_copyright(FILE_LIST "${CURRENT_PORT_DIR}/AMF-LICENSE.txt")
