//! The values of the Metal enumerations and the layouts of the Metal
//! structures the module passes, copied from the framework's headers: each
//! constant carries the name it stands for. Every enumeration here is an
//! `NSUInteger` (64 bits) except the GPU families, an `NSInteger`.
//! Throwaway, never product code.

/// `MTLPixelFormatR32Uint`.
pub const PIXEL_FORMAT_R32_UINT: usize = 53;
/// `MTLPixelFormatRGBA8Unorm`.
pub const PIXEL_FORMAT_RGBA8_UNORM: usize = 70;
/// `MTLPixelFormatRGBA8Unorm_sRGB`.
pub const PIXEL_FORMAT_RGBA8_UNORM_SRGB: usize = 71;
/// `MTLVertexFormatFloat2` and `MTLVertexFormatFloat4`.
pub const VERTEX_FORMAT_FLOAT2: usize = 29;
pub const VERTEX_FORMAT_FLOAT4: usize = 31;
/// `MTLPixelFormatRGBA16Float`.
pub const PIXEL_FORMAT_RGBA16_FLOAT: usize = 115;
/// `MTLPixelFormatDepth32Float`.
pub const PIXEL_FORMAT_DEPTH32_FLOAT: usize = 252;

/// `MTLTextureType2D`.
pub const TEXTURE_TYPE_2D: usize = 2;
/// `MTLTextureType2DMultisample`.
pub const TEXTURE_TYPE_2D_MULTISAMPLE: usize = 4;

/// `MTLTextureUsageShaderRead`.
pub const TEXTURE_USAGE_SHADER_READ: usize = 0x01;
/// `MTLTextureUsageShaderWrite`.
pub const TEXTURE_USAGE_SHADER_WRITE: usize = 0x02;
/// `MTLTextureUsageRenderTarget`.
pub const TEXTURE_USAGE_RENDER_TARGET: usize = 0x04;
/// `MTLTextureUsagePixelFormatView`.
pub const TEXTURE_USAGE_PIXEL_FORMAT_VIEW: usize = 0x10;

/// `MTLStorageModeShared`.
pub const STORAGE_MODE_SHARED: usize = 0;
/// `MTLStorageModePrivate`.
pub const STORAGE_MODE_PRIVATE: usize = 2;
/// `MTLResourceStorageModeShared`: the storage mode shifted left by four bits.
pub const RESOURCE_STORAGE_MODE_SHARED: usize = STORAGE_MODE_SHARED << 4;

/// `MTLLoadActionLoad`.
pub const LOAD_ACTION_LOAD: usize = 1;
/// `MTLLoadActionClear`.
pub const LOAD_ACTION_CLEAR: usize = 2;
/// `MTLStoreActionDontCare`.
pub const STORE_ACTION_DONT_CARE: usize = 0;
/// `MTLStoreActionStore`.
pub const STORE_ACTION_STORE: usize = 1;
/// `MTLStoreActionMultisampleResolve`.
pub const STORE_ACTION_MULTISAMPLE_RESOLVE: usize = 2;

/// `MTLPrimitiveTypeTriangle`.
pub const PRIMITIVE_TYPE_TRIANGLE: usize = 3;

/// `MTLCompareFunctionLessEqual`.
pub const COMPARE_FUNCTION_LESS_EQUAL: usize = 3;

/// `MTLBlendFactorOne`.
pub const BLEND_FACTOR_ONE: usize = 1;
/// `MTLBlendFactorSourceAlpha`.
pub const BLEND_FACTOR_SOURCE_ALPHA: usize = 4;
/// `MTLBlendFactorOneMinusSourceAlpha`.
pub const BLEND_FACTOR_ONE_MINUS_SOURCE_ALPHA: usize = 5;
/// `MTLBlendOperationAdd`.
pub const BLEND_OPERATION_ADD: usize = 0;

/// `MTLSamplerMinMagFilterNearest`.
pub const SAMPLER_MIN_MAG_FILTER_NEAREST: usize = 0;
/// `MTLSamplerMinMagFilterLinear`.
pub const SAMPLER_MIN_MAG_FILTER_LINEAR: usize = 1;
/// `MTLSamplerMipFilterNearest`.
pub const SAMPLER_MIP_FILTER_NEAREST: usize = 1;
/// `MTLSamplerMipFilterLinear`.
pub const SAMPLER_MIP_FILTER_LINEAR: usize = 2;
/// `MTLSamplerAddressModeClampToEdge`.
pub const SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE: usize = 0;
/// `MTLSamplerAddressModeRepeat`.
pub const SAMPLER_ADDRESS_MODE_REPEAT: usize = 2;

/// `MTLCommandBufferStatusCompleted`.
pub const COMMAND_BUFFER_STATUS_COMPLETED: usize = 4;
/// `MTLCommandBufferStatusError`.
pub const COMMAND_BUFFER_STATUS_ERROR: usize = 5;
/// `MTLCommandBufferStatusNotEnqueued` to `MTLCommandBufferStatusError`, by value.
pub const COMMAND_BUFFER_STATUS_NAMES: [&str; 6] = ["not enqueued", "enqueued", "committed", "scheduled", "completed", "error"];

/// `MTLGPUFamilyApple1`; the Apple families follow it one by one, up to
/// `MTLGPUFamilyApple9` (1009) in the headers this module was written from.
pub const GPU_FAMILY_APPLE_1: isize = 1001;
pub const GPU_FAMILY_APPLE_LAST_KNOWN: usize = 9;
/// `MTLGPUFamilyMac2`.
pub const GPU_FAMILY_MAC_2: isize = 2002;
/// `MTLGPUFamilyCommon1`; `MTLGPUFamilyCommon2` and `MTLGPUFamilyCommon3` follow it.
pub const GPU_FAMILY_COMMON_1: isize = 3001;
pub const GPU_FAMILY_COMMON_LAST_KNOWN: usize = 3;
/// `MTLGPUFamilyMetal3`.
pub const GPU_FAMILY_METAL_3: isize = 5001;

/// `MTLArgumentBuffersTier2`.
pub const ARGUMENT_BUFFERS_TIER_2: usize = 1;
/// `MTLReadWriteTextureTierNone`, `MTLReadWriteTextureTier1` and `MTLReadWriteTextureTier2`, by value.
pub const READ_WRITE_TEXTURE_TIER_NAMES: [&str; 3] = ["none", "tier 1", "tier 2"];

/// `MTLCounterSamplingPointAtStageBoundary`.
pub const COUNTER_SAMPLING_POINT_AT_STAGE_BOUNDARY: usize = 0;
/// `MTLCounterSamplingPointAtStageBoundary` to `MTLCounterSamplingPointAtBlitBoundary`, by value.
pub const COUNTER_SAMPLING_POINT_NAMES: [&str; 5] = ["stage boundary", "draw boundary", "dispatch boundary", "tile dispatch boundary", "blit boundary"];
/// `MTLCounterDontSample`.
pub const COUNTER_DONT_SAMPLE: usize = usize::MAX;
/// `MTLCounterErrorValue`.
pub const COUNTER_ERROR_VALUE: u64 = u64::MAX;

/// `MTLClearColor`: four doubles, passed in floating-point registers on arm64.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ClearColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

/// `MTLViewport`: six doubles.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Viewport {
    pub origin_x: f64,
    pub origin_y: f64,
    pub width: f64,
    pub height: f64,
    pub near: f64,
    pub far: f64,
}

/// `MTLScissorRect`: four `NSUInteger`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScissorRect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// `MTLOrigin`: three `NSUInteger`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Origin {
    pub x: usize,
    pub y: usize,
    pub z: usize,
}

/// `MTLSize`: three `NSUInteger`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Size {
    pub width: usize,
    pub height: usize,
    pub depth: usize,
}

/// `MTLRegion`: an origin and a size.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Region {
    pub origin: Origin,
    pub size: Size,
}

/// `NSRange`: a location and a length, two `NSUInteger`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Range {
    pub location: usize,
    pub length: usize,
}

/// `MTLSamplePosition`: two floats in 0..1 from the pixel's top-left corner.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SamplePosition {
    pub x: f32,
    pub y: f32,
}

impl Region {
    /// A whole level of a two-dimensional texture.
    pub fn of_level(width: usize, height: usize) -> Region {
        Region { origin: Origin { x: 0, y: 0, z: 0 }, size: Size { width, height, depth: 1 } }
    }
}

impl Viewport {
    /// A rectangle of the target with the depth range 0 to 1.
    pub fn of(x: usize, y: usize, width: usize, height: usize) -> Viewport {
        Viewport { origin_x: x as f64, origin_y: y as f64, width: width as f64, height: height as f64, near: 0.0, far: 1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes the framework's headers give these structures on a 64-bit Mac.
    #[test]
    fn the_structures_have_the_sizes_of_the_headers() {
        assert_eq!(std::mem::size_of::<ClearColor>(), 32);
        assert_eq!(std::mem::size_of::<Viewport>(), 48);
        assert_eq!(std::mem::size_of::<ScissorRect>(), 32);
        assert_eq!(std::mem::size_of::<Origin>(), 24);
        assert_eq!(std::mem::size_of::<Size>(), 24);
        assert_eq!(std::mem::size_of::<Region>(), 48);
        assert_eq!(std::mem::size_of::<Range>(), 16);
        assert_eq!(std::mem::size_of::<SamplePosition>(), 8);
    }
}
