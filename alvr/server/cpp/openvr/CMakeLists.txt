# Vendored prebuilt OpenVR SDK (headers + client stub library).
# Exposed as openvr::openvr_api. Imported-target interface includes are
# treated as SYSTEM, which silences the many warnings in these headers.

if(APPLE)
    # No prebuilt library is vendored for macOS; headers only.
    add_library(openvr_api INTERFACE)
    target_include_directories(openvr_api SYSTEM INTERFACE
        "${CMAKE_CURRENT_SOURCE_DIR}/headers")
else()
    add_library(openvr_api SHARED IMPORTED GLOBAL)
    set_target_properties(openvr_api PROPERTIES
        INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/headers")

    if(WIN32)
        set_target_properties(openvr_api PROPERTIES
            IMPORTED_IMPLIB   "${CMAKE_CURRENT_SOURCE_DIR}/lib/openvr_api.lib"
            IMPORTED_LOCATION "${CMAKE_CURRENT_SOURCE_DIR}/../bin/windows/openvr_api.dll")
    else()
        set_target_properties(openvr_api PROPERTIES
            IMPORTED_LOCATION "${CMAKE_CURRENT_SOURCE_DIR}/lib/libopenvr_api.so")
    endif()
endif()

add_library(openvr::openvr_api ALIAS openvr_api)
