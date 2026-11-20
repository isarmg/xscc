vcpkg_extract_source_archive(SOURCE_PATH
    ARCHIVE "${CURRENT_PORT_DIR}/amd-media-headers.tar.gz"
)
file(INSTALL "${SOURCE_PATH}/AMF/" DESTINATION "${CURRENT_PACKAGES_DIR}/include/AMF")
vcpkg_install_copyright(FILE_LIST "${CURRENT_PORT_DIR}/LICENSE.txt")
