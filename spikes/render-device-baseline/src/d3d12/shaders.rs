//! The HLSL sources of the spike compiled at run time by the system's
//! `D3DCompile` to the shader model 5.1 targets. Throwaway, never product code.

use std::ffi::c_void;
use std::fs;
use std::path::Path;
use std::ptr;

use super::ffi::*;
use super::gpu::{blob_bytes, blob_text};
use crate::loader::c_string;

/// The bytecode of every entry point the module draws or dispatches with.
#[derive(Clone)]
pub struct Shaders {
    pub scene_vertex: Vec<u8>,
    pub scene_pixel: Vec<u8>,
    pub compute: Vec<u8>,
    pub pass_through_vertex: Vec<u8>,
    pub pass_through_pixel: Vec<u8>,
    pub lit_forward_vertex: Vec<u8>,
    pub lit_forward_pixel: Vec<u8>,
    pub lit_geometry_vertex: Vec<u8>,
    pub lit_fill_four: Vec<u8>,
    pub lit_fill_five: Vec<u8>,
    pub lit_resolve_vertex: Vec<u8>,
    pub lit_resolve_pixel: Vec<u8>,
}

struct Compiler<'a> {
    compile: Compile,
    directory: &'a Path,
}

impl Compiler<'_> {
    fn entry(&self, file: &str, entry: &str, target: &str) -> Result<Vec<u8>, String> {
        let path = self.directory.join(file);
        let source = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let (name, entry_name, target_name) = (c_string(&path.display().to_string()), c_string(entry), c_string(target));
        let (mut code, mut errors): (Object, Object) = (ptr::null_mut(), ptr::null_mut());
        // SAFETY: the function is D3DCompile; every string is NUL-terminated
        // and live for the call; it writes two blobs owned below.
        let result = unsafe {
            (self.compile)(
                source.as_ptr() as *const c_void,
                source.len(),
                name.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                entry_name.as_ptr(),
                target_name.as_ptr(),
                COMPILE_ENABLE_STRICTNESS,
                0,
                &mut code,
                &mut errors,
            )
        };
        let messages = if errors.is_null() { String::new() } else { blob_text(&Com::from_raw(errors)) };
        let code = created(result, code, &format!("D3DCompile of {file} {entry} to {target}: {messages}"))?;
        Ok(blob_bytes(&code))
    }
}

impl Shaders {
    pub fn compile(compile: Compile, directory: &Path) -> Result<Shaders, String> {
        let compiler = Compiler { compile, directory };
        Ok(Shaders {
            scene_vertex: compiler.entry("scene.hlsl", "vs_main", "vs_5_1")?,
            scene_pixel: compiler.entry("scene.hlsl", "ps_main", "ps_5_1")?,
            compute: compiler.entry("compute.hlsl", "cs_main", "cs_5_1")?,
            pass_through_vertex: compiler.entry("pass_through.hlsl", "vs_main", "vs_5_1")?,
            pass_through_pixel: compiler.entry("pass_through.hlsl", "ps_main", "ps_5_1")?,
            lit_forward_vertex: compiler.entry("lit_forward.hlsl", "vs_main", "vs_5_1")?,
            lit_forward_pixel: compiler.entry("lit_forward.hlsl", "ps_main", "ps_5_1")?,
            lit_geometry_vertex: compiler.entry("lit_geometry.hlsl", "vs_main", "vs_5_1")?,
            lit_fill_four: compiler.entry("lit_geometry.hlsl", "ps_fill_four", "ps_5_1")?,
            lit_fill_five: compiler.entry("lit_geometry.hlsl", "ps_fill_five", "ps_5_1")?,
            lit_resolve_vertex: compiler.entry("lit_resolve.hlsl", "vs_main", "vs_5_1")?,
            lit_resolve_pixel: compiler.entry("lit_resolve.hlsl", "ps_main", "ps_5_1")?,
        })
    }
}
