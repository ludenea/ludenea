//! The lit scene on Direct3D 12: lit forward into one 16-bit float image, or
//! a geometry buffer of four or five attachments filled with no lighting and,
//! for one variant, lit by a second pass that loads it texel by texel. Each
//! repetition is timed from the start of the recording to the completion of
//! the device's work. Throwaway, never product code.

use std::time::Instant;

use super::ffi::*;
use super::gpu::*;
use super::shaders::Shaders;
use crate::backend::{LitRender, LitVariant};
use crate::loader::c_string;
use crate::scene::{self, Scene};

const VERTEX_STRIDE: u32 = 52;
const SIDE: u32 = scene::LIT_SIZE;
const LIT_CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// A constant buffer bound as a root view is read in blocks of 256 bytes.
const CONSTANT_BLOCK: usize = 256;

/// The attachments the scene's pass writes, in their order.
fn fill_formats(variant: LitVariant) -> Vec<u32> {
    match variant {
        LitVariant::Forward => vec![FORMAT_R16G16B16A16_FLOAT],
        LitVariant::GeometryBuffer160 | LitVariant::GeometryBuffer160Lit => vec![FORMAT_R8G8B8A8_UNORM, FORMAT_R8G8B8A8_UNORM, FORMAT_R8G8B8A8_UNORM, FORMAT_R16G16B16A16_FLOAT],
        LitVariant::GeometryBuffer192 => vec![FORMAT_R8G8B8A8_UNORM, FORMAT_R8G8B8A8_UNORM, FORMAT_R8G8B8A8_UNORM, FORMAT_R16G16B16A16_FLOAT, FORMAT_R8G8B8A8_UNORM],
    }
}

fn fill_clear(variant: LitVariant) -> [f32; 4] {
    if variant == LitVariant::Forward { LIT_CLEAR } else { [0.0; 4] }
}

/// The second pass of the lit geometry buffer: its target, the views of the
/// four attachments and its pipeline.
struct LightingPass {
    pipeline: Com,
    root_signature: Com,
    views: DescriptorHeap,
    target: Com,
}

/// Everything one variant is drawn with, created before the first repetition.
struct LitResources {
    lighting: Option<LightingPass>,
    pipeline: Com,
    root_signature: Com,
    target_views: DescriptorHeap,
    depth_views: DescriptorHeap,
    constants: Com,
    vertex_buffer: Com,
    /// Held for its view in `depth_views`.
    _depth: Com,
    targets: Vec<Com>,
    clear: [f32; 4],
    vertex_bytes: u32,
    vertex_count: u32,
}

impl LitResources {
    fn create(gpu: &mut Gpu, shaders: &Shaders, scene: &Scene, variant: LitVariant) -> Result<LitResources, String> {
        let formats = fill_formats(variant);
        let clear = fill_clear(variant);
        let targets = formats
            .iter()
            .map(|format| {
                let desc = texture_desc(SIDE, SIDE, 1, *format, ONE_SAMPLE, RESOURCE_FLAG_ALLOW_RENDER_TARGET);
                gpu.committed(HEAP_TYPE_DEFAULT, &desc, STATE_RENDER_TARGET, Some(&clear_color(*format, clear)), "lit attachment")
            })
            .collect::<Result<Vec<Com>, String>>()?;
        let depth_desc = texture_desc(SIDE, SIDE, 1, FORMAT_D32_FLOAT, ONE_SAMPLE, RESOURCE_FLAG_ALLOW_DEPTH_STENCIL);
        let depth = gpu.committed(HEAP_TYPE_DEFAULT, &depth_desc, STATE_DEPTH_WRITE, Some(&clear_depth_value(1.0)), "lit depth")?;
        let target_views = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_RTV, formats.len() as u32 + 1, false)?;
        for (index, target) in targets.iter().enumerate() {
            gpu.render_target_view(target.raw(), FORMAT_UNKNOWN, target_views.cpu(index as u32));
        }
        let depth_views = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_DSV, 1, false)?;
        gpu.depth_view(depth.raw(), depth_views.cpu(0));

        let vertex_bytes = scene.lit_vertex_bytes();
        let vertex_buffer = gpu.upload_buffer(vertex_bytes, STATE_VERTEX_AND_CONSTANT_BUFFER, RESOURCE_FLAG_NONE, "lit vertices")?;
        let mut constant_bytes = scene.lit_constant_bytes().to_vec();
        constant_bytes.resize(constant_bytes.len().next_multiple_of(CONSTANT_BLOCK), 0);
        let constants = gpu.upload_buffer(&constant_bytes, STATE_VERTEX_AND_CONSTANT_BUFFER, RESOURCE_FLAG_NONE, "lit constants")?;

        let root_signature = gpu.root_signature(&[RootParameter::descriptor(ROOT_PARAMETER_TYPE_CBV, 0, SHADER_VISIBILITY_ALL)], &[], ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, "lit fill")?;
        let pipeline = fill_pipeline(gpu, shaders, root_signature.raw(), variant, &formats)?;
        let lighting = if variant == LitVariant::GeometryBuffer160Lit {
            let lighting = LightingPass::create(gpu, shaders, &targets, &formats)?;
            gpu.render_target_view(lighting.target.raw(), FORMAT_UNKNOWN, target_views.cpu(formats.len() as u32));
            Some(lighting)
        } else {
            None
        };
        Ok(LitResources {
            lighting,
            pipeline,
            root_signature,
            target_views,
            depth_views,
            constants,
            vertex_buffer,
            _depth: depth,
            targets,
            clear,
            vertex_bytes: vertex_bytes.len() as u32,
            vertex_count: scene.lit_vertices.len() as u32,
        })
    }

    /// The texture the variant's output is read from, with its texel size.
    fn output(&self) -> (Object, u32) {
        match &self.lighting {
            Some(lighting) => (lighting.target.raw(), 8),
            None if self.targets.len() == 1 => (self.targets[0].raw(), 8),
            None => (self.targets[0].raw(), 4),
        }
    }

    fn record(&self, list: Object) {
        let handles: Vec<CpuHandle> = (0..self.targets.len() as u32).map(|index| self.target_views.cpu(index)).collect();
        let constants = gpu_address(self.constants.raw());
        set_graphics_root_signature(list, self.root_signature.raw());
        set_graphics_constants(list, constants);
        set_viewport(list, 0, SIDE, SIDE);
        for handle in &handles {
            clear_target(list, *handle, self.clear);
        }
        clear_depth(list, self.depth_views.cpu(0), 1.0);
        set_render_targets(list, &handles, Some(self.depth_views.cpu(0)));
        set_vertex_buffer(list, self.vertex_buffer.raw(), self.vertex_bytes, VERTEX_STRIDE);
        set_pipeline(list, self.pipeline.raw());
        draw(list, self.vertex_count, 0);
        if let Some(lighting) = &self.lighting {
            self.record_lighting(list, lighting, constants);
        }
    }

    fn record_lighting(&self, list: Object, lighting: &LightingPass, constants: u64) {
        let to_read: Vec<ResourceBarrier> = self.targets.iter().map(|target| transition(target.raw(), STATE_RENDER_TARGET, STATE_PIXEL_SHADER_RESOURCE)).collect();
        barriers(list, &to_read);
        let lit_target = self.target_views.cpu(self.targets.len() as u32);
        set_descriptor_heaps(list, &[lighting.views.heap.raw()]);
        set_graphics_root_signature(list, lighting.root_signature.raw());
        set_graphics_constants(list, constants);
        set_graphics_table(list, 1, lighting.views.gpu(0));
        clear_target(list, lit_target, LIT_CLEAR);
        set_render_targets(list, &[lit_target], None);
        set_triangle_list(list);
        set_pipeline(list, lighting.pipeline.raw());
        draw(list, 3, 0);
        let to_write: Vec<ResourceBarrier> = self.targets.iter().map(|target| transition(target.raw(), STATE_PIXEL_SHADER_RESOURCE, STATE_RENDER_TARGET)).collect();
        barriers(list, &to_write);
    }
}

fn set_graphics_constants(list: Object, address: u64) {
    // SAFETY: the list is recording with a root signature whose first
    // parameter is a constant buffer view; the buffer is live.
    unsafe { com!(list, slot::list::SET_GRAPHICS_ROOT_CONSTANT_BUFFER_VIEW, fn(u32, u64) -> (), 0, address) };
}

impl LightingPass {
    fn create(gpu: &Gpu, shaders: &Shaders, attachments: &[Com], formats: &[u32]) -> Result<LightingPass, String> {
        let desc = texture_desc(SIDE, SIDE, 1, FORMAT_R16G16B16A16_FLOAT, ONE_SAMPLE, RESOURCE_FLAG_ALLOW_RENDER_TARGET);
        let target = gpu.committed(HEAP_TYPE_DEFAULT, &desc, STATE_RENDER_TARGET, Some(&clear_color(FORMAT_R16G16B16A16_FLOAT, LIT_CLEAR)), "lit image")?;
        let views = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, attachments.len() as u32, true)?;
        for (index, (attachment, format)) in attachments.iter().zip(formats).enumerate() {
            gpu.texture_view(attachment.raw(), *format, 1, views.cpu(index as u32));
        }
        let range = [DescriptorRange { kind: DESCRIPTOR_RANGE_TYPE_SRV, count: attachments.len() as u32, base_register: 0, space: 0, offset_from_table_start: 0 }];
        let parameters = [RootParameter::descriptor(ROOT_PARAMETER_TYPE_CBV, 0, SHADER_VISIBILITY_PIXEL), RootParameter::table(&range, SHADER_VISIBILITY_PIXEL)];
        let root_signature = gpu.root_signature(&parameters, &[], 0, "lit lighting pass")?;
        let mut pipeline_desc = default_graphics_desc(root_signature.raw(), &shaders.lit_resolve_vertex, &shaders.lit_resolve_pixel);
        set_targets(&mut pipeline_desc, &[FORMAT_R16G16B16A16_FLOAT]);
        let pipeline = gpu.graphics_pipeline(&pipeline_desc, "lit lighting pass")?;
        Ok(LightingPass { pipeline, root_signature, views, target })
    }
}

fn fill_pipeline(gpu: &Gpu, shaders: &Shaders, root_signature: Object, variant: LitVariant, formats: &[u32]) -> Result<Com, String> {
    let (position, world, normal, albedo) = (c_string("POSITION"), c_string("TEXCOORD"), c_string("NORMAL"), c_string("COLOR"));
    let elements = [
        input_element(&position, FORMAT_R32G32B32A32_FLOAT, 0),
        input_element(&world, FORMAT_R32G32B32_FLOAT, 16),
        input_element(&normal, FORMAT_R32G32B32_FLOAT, 28),
        input_element(&albedo, FORMAT_R32G32B32_FLOAT, 40),
    ];
    let (vertex, pixel) = match variant {
        LitVariant::Forward => (&shaders.lit_forward_vertex, &shaders.lit_forward_pixel),
        LitVariant::GeometryBuffer192 => (&shaders.lit_geometry_vertex, &shaders.lit_fill_five),
        LitVariant::GeometryBuffer160 | LitVariant::GeometryBuffer160Lit => (&shaders.lit_geometry_vertex, &shaders.lit_fill_four),
    };
    let mut desc = default_graphics_desc(root_signature, vertex, pixel);
    desc.input_layout = InputLayoutDesc { elements: elements.as_ptr(), count: elements.len() as u32 };
    set_targets(&mut desc, formats);
    set_depth(&mut desc, true);
    gpu.graphics_pipeline(&desc, &format!("lit fill, {}", variant.name()))
}

pub fn render(gpu: &mut Gpu, shaders: &Shaders, scene: &Scene, variant: LitVariant, repetitions: usize) -> Result<LitRender, String> {
    let resources = LitResources::create(gpu, shaders, scene, variant)?;
    let recorder = gpu.recorder()?;
    let mut microseconds = Vec::with_capacity(repetitions);
    for _ in 0..repetitions {
        let started = Instant::now();
        recorder.begin()?;
        resources.record(recorder.list.raw());
        close(recorder.list.raw())?;
        let value = gpu.execute(&[recorder.list.raw()])?;
        gpu.wait(value)?;
        microseconds.push(started.elapsed().as_secs_f64() * 1_000_000.0);
    }
    let (output, bytes_per_texel) = resources.output();
    let format = if bytes_per_texel == 8 { FORMAT_R16G16B16A16_FLOAT } else { FORMAT_R8G8B8A8_UNORM };
    let read_back = gpu.read_back_for(&texture_desc(SIDE, SIDE, 1, format, ONE_SAMPLE, RESOURCE_FLAG_ALLOW_RENDER_TARGET), "lit read-back")?;
    gpu.submit_and_wait(|list| {
        barriers(list, &[transition(output, STATE_RENDER_TARGET, STATE_COPY_SOURCE)]);
        copy_texture_to_read_back(list, output, &read_back);
        barriers(list, &[transition(output, STATE_COPY_SOURCE, STATE_RENDER_TARGET)]);
    })?;
    Ok(LitRender { microseconds, output: tight_rows(&read_back, bytes_per_texel)? })
}
