//! The compute and the pass-through exercises on Direct3D 12. Throwaway,
//! never product code.

use std::ffi::c_void;
use std::ptr;

use super::ffi::*;
use super::gpu::*;
use super::shaders::Shaders;
use crate::backend::ComputeOutput;
use crate::scene::{self, Image, Scene};

const PASS_THROUGH_SIDE: u32 = scene::PASS_THROUGH_SIDE;

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes.as_chunks::<4>().0.iter().map(|word| u32::from_le_bytes(*word)).collect()
}

/// One dispatch of 8 by 8 groups: the input as a root shader resource view,
/// the output buffer as a root unordered access view, the storage texture in
/// a table of one unordered access view; both outputs read back.
pub fn compute(gpu: &mut Gpu, shaders: &Shaders, scene: &Scene) -> Result<ComputeOutput, String> {
    let texture_range = [DescriptorRange { kind: DESCRIPTOR_RANGE_TYPE_UAV, count: 1, base_register: 1, space: 0, offset_from_table_start: 0 }];
    let parameters = [
        RootParameter::descriptor(ROOT_PARAMETER_TYPE_SRV, 0, SHADER_VISIBILITY_ALL),
        RootParameter::descriptor(ROOT_PARAMETER_TYPE_UAV, 0, SHADER_VISIBILITY_ALL),
        RootParameter::table(&texture_range, SHADER_VISIBILITY_ALL),
    ];
    let root_signature = gpu.root_signature(&parameters, &[], 0, "compute")?;
    let pipeline = gpu.compute_pipeline(root_signature.raw(), &shaders.compute, "compute")?;

    let input = gpu.upload_buffer(scene.compute_input_bytes(), STATE_NON_PIXEL_SHADER_RESOURCE, RESOURCE_FLAG_NONE, "compute input")?;
    let size = (scene::COMPUTE_ELEMENTS * 4) as u64;
    let output = gpu.buffer(HEAP_TYPE_DEFAULT, size, RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, "compute output buffer")?;
    let side = scene::COMPUTE_SIDE;
    let texture_desc = texture_desc(side, side, 1, FORMAT_R32_UINT, ONE_SAMPLE, RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    let texture = gpu.committed(HEAP_TYPE_DEFAULT, &texture_desc, STATE_UNORDERED_ACCESS, None, "compute storage texture")?;
    let views = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1, true)?;
    // SAFETY: the device and the texture (typed R32_UINT, so no description
    // is needed) are live; the handle is a slot of the heap.
    unsafe { com!(gpu.raw(), slot::device::CREATE_UNORDERED_ACCESS_VIEW, fn(Object, Object, *const c_void, CpuHandle) -> (), texture.raw(), ptr::null_mut(), ptr::null(), views.cpu(0)) };
    let buffer_back = gpu.buffer(HEAP_TYPE_READBACK, size, RESOURCE_FLAG_NONE, "compute buffer read-back")?;
    let texture_back = gpu.read_back_for(&texture_desc, "compute texture read-back")?;

    let (input_address, output_address) = (gpu_address(input.raw()), gpu_address(output.raw()));
    let (output_buffer, texture_object, buffer_back_object) = (output.raw(), texture.raw(), buffer_back.raw());
    let groups = side / 8;
    gpu.submit_and_wait(|list| {
        barriers(list, &[transition(output_buffer, STATE_COMMON, STATE_UNORDERED_ACCESS)]);
        set_descriptor_heaps(list, &[views.heap.raw()]);
        // SAFETY: the list is recording; the root signature, the pipeline and
        // the buffers are live and in the states their bindings need.
        unsafe {
            com!(list, slot::list::SET_COMPUTE_ROOT_SIGNATURE, fn(Object) -> (), root_signature.raw());
            com!(list, slot::list::SET_COMPUTE_ROOT_SHADER_RESOURCE_VIEW, fn(u32, u64) -> (), 0, input_address);
            com!(list, slot::list::SET_COMPUTE_ROOT_UNORDERED_ACCESS_VIEW, fn(u32, u64) -> (), 1, output_address);
            com!(list, slot::list::SET_COMPUTE_ROOT_DESCRIPTOR_TABLE, fn(u32, u64) -> (), 2, views.gpu(0));
        }
        set_pipeline(list, pipeline.raw());
        // SAFETY: the list is recording with a compute pipeline bound.
        unsafe { com!(list, slot::list::DISPATCH, fn(u32, u32, u32) -> (), groups, groups, 1) };
        barriers(list, &[transition(output_buffer, STATE_UNORDERED_ACCESS, STATE_COPY_SOURCE), transition(texture_object, STATE_UNORDERED_ACCESS, STATE_COPY_SOURCE)]);
        // SAFETY: the list is recording; both buffers hold `size` bytes.
        unsafe { com!(list, slot::list::COPY_BUFFER_REGION, fn(Object, u64, Object, u64, u64) -> (), buffer_back_object, 0, output_buffer, 0, size) };
        copy_texture_to_read_back(list, texture_object, &texture_back);
    })?;
    let buffer = words(&read_buffer(buffer_back.raw(), size)?);
    let texture = words(&tight_rows(&texture_back, 4)?);
    Ok(ComputeOutput { buffer, texture })
}

/// A typeless source and a typeless target, each with a plain and an sRGB
/// view: the left half drawn through the plain views, the right half through
/// the sRGB views.
pub fn pass_through(gpu: &mut Gpu, shaders: &Shaders, scene: &Scene) -> Result<Image, String> {
    let side = PASS_THROUGH_SIDE;
    let source_desc = texture_desc(side, side, 1, FORMAT_R8G8B8A8_TYPELESS, ONE_SAMPLE, RESOURCE_FLAG_NONE);
    let source = gpu.upload_texture(&source_desc, std::slice::from_ref(&scene.pass_through_source), 4, STATE_PIXEL_SHADER_RESOURCE, "pass-through source")?;
    let target_desc = texture_desc(side * 2, side, 1, FORMAT_R8G8B8A8_TYPELESS, ONE_SAMPLE, RESOURCE_FLAG_ALLOW_RENDER_TARGET);
    let target = gpu.committed(HEAP_TYPE_DEFAULT, &target_desc, STATE_RENDER_TARGET, Some(&clear_color(FORMAT_R8G8B8A8_UNORM, [0.0; 4])), "pass-through target")?;
    let read_back = gpu.read_back_for(&target_desc, "pass-through read-back")?;

    let forms = [FORMAT_R8G8B8A8_UNORM, FORMAT_R8G8B8A8_UNORM_SRGB];
    let views = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 2, true)?;
    let targets = gpu.descriptor_heap(DESCRIPTOR_HEAP_TYPE_RTV, 2, false)?;
    for (index, format) in forms.iter().enumerate() {
        gpu.texture_view(source.raw(), *format, 1, views.cpu(index as u32));
        gpu.render_target_view(target.raw(), *format, targets.cpu(index as u32));
    }
    let range = [DescriptorRange { kind: DESCRIPTOR_RANGE_TYPE_SRV, count: 1, base_register: 0, space: 0, offset_from_table_start: 0 }];
    let nearest = StaticSamplerDesc {
        filter: FILTER_MIN_MAG_MIP_POINT,
        address_u: ADDRESS_MODE_CLAMP,
        address_v: ADDRESS_MODE_CLAMP,
        address_w: ADDRESS_MODE_CLAMP,
        mip_lod_bias: 0.0,
        max_anisotropy: 1,
        comparison: COMPARISON_NEVER,
        border_color: 0,
        min_lod: 0.0,
        max_lod: FLOAT32_MAX,
        register: 0,
        space: 0,
        visibility: SHADER_VISIBILITY_PIXEL,
    };
    let root_signature = gpu.root_signature(&[RootParameter::table(&range, SHADER_VISIBILITY_PIXEL)], &[nearest], 0, "pass-through")?;
    let pipelines = forms
        .iter()
        .map(|format| {
            let mut desc = default_graphics_desc(root_signature.raw(), &shaders.pass_through_vertex, &shaders.pass_through_pixel);
            set_targets(&mut desc, &[*format]);
            gpu.graphics_pipeline(&desc, "pass-through")
        })
        .collect::<Result<Vec<Com>, String>>()?;

    let target_object = target.raw();
    gpu.submit_and_wait(|list| {
        clear_target(list, targets.cpu(0), [0.0; 4]);
        set_descriptor_heaps(list, &[views.heap.raw()]);
        set_graphics_root_signature(list, root_signature.raw());
        set_triangle_list(list);
        for (index, pipeline) in pipelines.iter().enumerate() {
            set_viewport(list, index as u32 * side, side, side);
            set_render_targets(list, &[targets.cpu(index as u32)], None);
            set_graphics_table(list, 0, views.gpu(index as u32));
            set_pipeline(list, pipeline.raw());
            draw(list, 3, 0);
        }
        barriers(list, &[transition(target_object, STATE_RENDER_TARGET, STATE_COPY_SOURCE)]);
        copy_texture_to_read_back(list, target_object, &read_back);
    })?;
    Ok(Image { width: side * 2, height: side, rgba: tight_rows(&read_back, 4)? })
}
