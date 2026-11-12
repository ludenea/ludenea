//! One open Direct3D 12 device as the spike's driver sees it: its
//! capabilities, the sample pattern of the filtering scene, the exercises and
//! the validation layer's messages. Throwaway, never product code.

use super::capabilities;
use super::exercises;
use super::filtering::{self, FilteringScene};
use super::gpu::Gpu;
use super::lit;
use super::shaders::Shaders;
use super::validation::{self, Collector};
use crate::backend::{Capability, ComputeOutput, Device, LitRender, LitVariant, Recording, SceneRender};
use crate::scene::{Image, Scene};

pub struct Direct3d12Device<'a> {
    /// Created on the first render of the filtering scene.
    scene: Option<FilteringScene>,
    collector: Option<Collector>,
    pattern: filtering::SamplePattern,
    shaders: std::borrow::Cow<'a, Shaders>,
    /// Dropped last: every object above is released before the device.
    gpu: Gpu,
}

impl<'a> Direct3d12Device<'a> {
    pub fn new(gpu: Gpu, shaders: std::borrow::Cow<'a, Shaders>, collector: Option<Collector>) -> Result<Direct3d12Device<'a>, String> {
        let pattern = filtering::SamplePattern::choose(&gpu)?;
        Ok(Direct3d12Device { scene: None, collector, pattern, shaders, gpu })
    }
}

impl Drop for Direct3d12Device<'_> {
    fn drop(&mut self) {
        // Every exercise waits for its work, so the queue is idle here unless
        // one failed half way; the resources are released after it drains.
        // A failure has no caller to report to; releasing is all that is left.
        let _ = self.gpu.wait_idle();
    }
}

impl Device for Direct3d12Device<'_> {
    fn capabilities(&mut self) -> Vec<Capability> {
        capabilities::read(&self.gpu)
    }

    fn settings(&self) -> String {
        format!(
            "{}; the three samplers in a shader-visible sampler heap, the five textures in a table of shader resource views; no environment setting is read by the interface",
            self.pattern.description
        )
    }

    fn render_scene(&mut self, scene: &Scene, recording: Recording) -> Result<SceneRender, String> {
        if self.scene.is_none() {
            self.scene = Some(FilteringScene::create(&mut self.gpu, &self.shaders, scene, self.pattern.sample)?);
        }
        match &self.scene {
            Some(resources) => resources.render(&mut self.gpu, scene, recording),
            None => Err("the filtering scene's resources were not created".to_string()),
        }
    }

    fn compute(&mut self, scene: &Scene) -> Result<ComputeOutput, String> {
        exercises::compute(&mut self.gpu, &self.shaders, scene)
    }

    fn pass_through(&mut self, scene: &Scene) -> Result<Image, String> {
        exercises::pass_through(&mut self.gpu, &self.shaders, scene)
    }

    fn lit(&mut self, scene: &Scene, variant: LitVariant, repetitions: usize) -> Result<LitRender, String> {
        lit::render(&mut self.gpu, &self.shaders, scene, variant, repetitions)
    }

    fn validation_delivery(&self) -> String {
        match &self.collector {
            Some(collector) => collector.delivery(),
            None => "the debug layer is not enabled on this device".to_string(),
        }
    }

    fn validation_messages(&mut self) -> Vec<String> {
        self.collector.as_mut().map(Collector::drain).unwrap_or_default()
    }

    fn misuse(&mut self) -> String {
        validation::misuse(self.gpu.raw())
    }
}
