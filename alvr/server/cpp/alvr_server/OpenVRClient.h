#pragma once

// Client-side OpenVR API (openvr.h) for use inside the driver DLL.
//
// This DLL is an OpenVR *driver* (openvr_driver.h), but a few features
// (chaperone setup) must go through the OpenVR *client* API (openvr.h).
// Both headers define inline helpers in namespace vr with identical mangled
// names but different bodies:
//
//     vr::VRSettings()  vr::VRResources()  vr::VRDriverManager()  vr::VRIOBuffer()
//
// The client flavor resolves through OpenVRInternal_ModuleContext() (valid
// only after vr::VR_Init), the driver flavor through
// OpenVRInternal_ModuleServerDriverContext(). Emitting the client flavor of
// any of these into the DLL is an ODR violation: the linker keeps a single
// copy per symbol, so in builds without inlining (MSVC /Ob0, -O0) every call
// in the DLL — including the driver's own vr::VRSettings() calls — can
// dispatch to the client flavor. The client context is not initialized at
// device-activation time, so vrserver dies with a null dereference in
// OvrHmd::Activate. (Historically dodged by always compiling this code in
// release, where the helpers are inlined at each call site.)
//
// Rule: translation units that need the client API include this header,
// never <openvr.h> directly, and reach the colliding interfaces only through
// the accessors below. The colliding helper names are poisoned underneath to
// enforce this at compile time.

#include <openvr.h>

// The one sanctioned way to get the *client* IVRSettings in this DLL.
// Only valid between a successful vr::VR_Init and vr::VR_Shutdown.
inline vr::IVRSettings *ClientVRSettings() {
    return vr::OpenVRInternal_ModuleContext().VRSettings();
}

// Compile-time tripwire: any use of the colliding helper names after this
// point is an error. (This must come after the accessors above: poisoning
// bans every use of the token, member calls included.)
#if defined(__GNUC__) || defined(__clang__)
#pragma GCC poison VRSettings VRResources VRDriverManager VRIOBuffer
#elif defined(_MSC_VER)
#pragma deprecated(VRSettings, VRResources, VRDriverManager, VRIOBuffer)
#pragma warning(error : 4995)
#endif
