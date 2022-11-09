#![allow(non_upper_case_globals, non_snake_case, clippy::missing_safety_doc)]
include!(concat!(env!("OUT_DIR"), "/alxr_engine.rs"));

impl From<&str> for crate::ALXRGraphicsApi {
    fn from(input: &str) -> Self {
        let trimmed = input.trim();
        match trimmed {
            "Vulkan2" => crate::ALXRGraphicsApi::Vulkan2,
            "Vulkan" => crate::ALXRGraphicsApi::Vulkan,
            "D3D12" => crate::ALXRGraphicsApi::D3D12,
            "D3D11" => crate::ALXRGraphicsApi::D3D11,
            "OpenGLES" => crate::ALXRGraphicsApi::OpenGLES,
            "OpenGL" => crate::ALXRGraphicsApi::OpenGL,
            _ => crate::ALXRGraphicsApi::Auto,
        }
    }
}

impl From<&str> for crate::ALXRDecoderType {
    fn from(input: &str) -> Self {
        let trimmed = input.trim();
        match trimmed {
            "D311VA" => crate::ALXRDecoderType::D311VA,
            "NVDEC" => crate::ALXRDecoderType::NVDEC,
            "CUVID" => crate::ALXRDecoderType::CUVID,
            "VAAPI" => crate::ALXRDecoderType::VAAPI,
            "CPU" => crate::ALXRDecoderType::CPU,
            #[cfg(target_os = "windows")]
            _ => crate::ALXRDecoderType::D311VA,
            #[cfg(not(target_os = "windows"))]
            _ => crate::ALXRDecoderType::VAAPI,
        }
    }
}

impl From<&str> for crate::ALXRColorSpace {
    fn from(input: &str) -> Self {
        let trimmed = input.trim();
        match trimmed {
            "Unmanaged" => crate::ALXRColorSpace::Unmanaged,
            "Rec709" => crate::ALXRColorSpace::Rec709,
            "RiftCV1" => crate::ALXRColorSpace::RiftCV1,
            "RiftS" => crate::ALXRColorSpace::RiftS,
            "Quest" => crate::ALXRColorSpace::Quest,
            "P3" => crate::ALXRColorSpace::P3,
            "AdobeRgb" => crate::ALXRColorSpace::AdobeRgb,
            "Rec2020" => crate::ALXRColorSpace::Rec2020,
            _ => crate::ALXRColorSpace::Rec2020,
        }
    }
}

impl ALXRSystemProperties {
    pub fn new() -> ALXRSystemProperties {
        ALXRSystemProperties {
            systemName: [0; 256],
            currentRefreshRate: 90.0,
            refreshRates: std::ptr::null(),
            refreshRatesCount: 0,
            recommendedEyeWidth: 0,
            recommendedEyeHeight: 0,
        }
    }
}

unsafe impl Send for ALXRGuardianData {}
