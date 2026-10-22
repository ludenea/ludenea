//! The window's surface and what draws into it. `Presenter` turns the outcomes of asking the
//! surface for a texture into actions: a frame drawn and presented, a frame skipped (a
//! zero-size or hidden window, a timeout), the surface configured again (lost or outdated),
//! or a failure with its cause. `Display` draws the match through the presenter with a
//! renderer, and replaces both on a new device once when the device is lost.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ludenea_app::project::{ProjectTexture, ProjectTextures};
use ludenea_draw::frame::RenderFrame;
use ludenea_render::atlas::{Atlas, TextureSource, frames_of, pack};
use ludenea_render::gpu::{Overlay, RenderError, Renderer, Size};

use crate::RuntimeError;

/// How long the surface may stay lost or outdated, configured again at every frame, before
/// the window gives up on it. A resize or a change of display makes it outdated for a few
/// frames; one that no configuration brings back within seconds will not come back, and
/// ending the session then writes the replay rather than leaving a frozen window.
const MAX_TIME_WITHOUT_SURFACE: Duration = Duration::from_secs(5);

/// The time now, for pacing the window's frames.
#[expect(
    clippy::disallowed_methods,
    reason = "runtime only: the time between displayed frames paces the host, and never feeds the simulation except through recorded inputs"
)]
pub(crate) fn now() -> Instant {
    Instant::now()
}

/// A texture to draw a frame into, and what presents it (none for a fake surface).
struct Frame {
    view: wgpu::TextureView,
    texture: Option<wgpu::SurfaceTexture>,
}

/// What asking the surface for a texture gave.
enum Acquired {
    /// A texture; `true` when the surface should be configured again after presenting it.
    Frame(Frame, bool),
    Timeout,
    Occluded,
    Outdated,
    Lost,
    Validation,
}

/// Where frames are drawn: the window's surface, or an offscreen fake in tests.
trait Target {
    fn configure(&mut self, width: u32, height: u32);
    fn acquire(&mut self) -> Acquired;
    fn present(&mut self, frame: Frame);
}

/// A window's wgpu surface.
struct WindowSurface {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl Target for WindowSurface {
    fn configure(&mut self, width: u32, height: u32) {
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn acquire(&mut self) -> Acquired {
        let (texture, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => (texture, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => (texture, true),
            wgpu::CurrentSurfaceTexture::Timeout => return Acquired::Timeout,
            wgpu::CurrentSurfaceTexture::Occluded => return Acquired::Occluded,
            wgpu::CurrentSurfaceTexture::Outdated => return Acquired::Outdated,
            wgpu::CurrentSurfaceTexture::Lost => return Acquired::Lost,
            wgpu::CurrentSurfaceTexture::Validation => return Acquired::Validation,
        };
        let view = texture.texture.create_view(&Default::default());
        Acquired::Frame(
            Frame {
                view,
                texture: Some(texture),
            },
            suboptimal,
        )
    }

    fn present(&mut self, frame: Frame) {
        if let Some(texture) = frame.texture {
            self.queue.present(texture);
        }
    }
}

/// Presents frames to a window's surface, keeping it configured to the window's size.
pub struct Presenter {
    target: Box<dyn Target>,
    format: wgpu::TextureFormat,
    /// The size the surface is configured to; a side of zero skips drawing.
    size: Size,
    /// The device's `max_texture_dimension_2d`, which the surface is kept within.
    max_side: u32,
    /// When the surface was first found lost or outdated, in the current run of such frames.
    lost_since: Option<Instant>,
    /// The last texture asked for was refused: the window is hidden, or its surface is being
    /// configured again.
    hidden: bool,
    /// How long the last frame waited for the surface's texture.
    waited: Duration,
    clock: Box<dyn Fn() -> Instant>,
}

impl Presenter {
    /// Presents to `surface` with `device`, in a format that stores colours as given (the
    /// renderer's pixels are already what the screen shows), in step with the display when
    /// `vsync` is on and as fast as possible when it is off. The surface is configured by the
    /// first `resize`, once a renderer holds the device's errors (wgpu panics on an error
    /// nothing holds).
    pub fn new(
        surface: wgpu::Surface<'static>,
        adapter: &wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        vsync: bool,
    ) -> Result<Presenter, RuntimeError> {
        let capabilities = surface.get_capabilities(adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .ok_or_else(|| {
                RuntimeError::Gpu(format!(
                    "the window offers no surface format that stores colours as given: it offers {:?}",
                    capabilities.formats
                ))
            })?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: 0,
            height: 0,
            present_mode: if vsync {
                wgpu::PresentMode::Fifo
            } else {
                wgpu::PresentMode::AutoNoVsync
            },
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: vec![],
            color_space: Default::default(),
        };
        let max_side = device.limits().max_texture_dimension_2d;
        let target = WindowSurface {
            surface,
            device,
            queue,
            config,
        };
        Ok(Presenter::with_target(
            Box::new(target),
            format,
            max_side,
            Box::new(now),
        ))
    }

    /// A presenter of zero size on `target`, timing a lost surface with `clock`.
    fn with_target(
        target: Box<dyn Target>,
        format: wgpu::TextureFormat,
        max_side: u32,
        clock: Box<dyn Fn() -> Instant>,
    ) -> Presenter {
        Presenter {
            target,
            format,
            size: Size {
                width: 0,
                height: 0,
            },
            max_side,
            lost_since: None,
            hidden: false,
            waited: Duration::ZERO,
            clock,
        }
    }

    /// The format of the surface's textures, which a renderer drawing into them must use.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Whether frames are drawn: not while the window has no pixels (minimized).
    pub fn can_draw(&self) -> bool {
        self.size.width > 0 && self.size.height > 0
    }

    /// Whether the last frame found no texture to draw into: the window is hidden (behind
    /// another window, on another desktop, or the screen locked), or its surface was lost or
    /// outdated and is configured again. The display no longer paces the loop then, so the
    /// caller paces it; `present` keeps asking, and draws once a texture comes.
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// How long the last frame waited for the surface's texture: the display hands one over
    /// once it has shown an earlier one, so a frame drawn faster than the display refreshes
    /// waits there.
    pub fn waited(&self) -> Duration {
        self.waited
    }

    /// Follows the window to `width` by `height` physical pixels, kept within the device's
    /// texture limit. At a side of zero, frames are skipped until the next resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        // A new configuration starts a new run of lost frames.
        self.lost_since = None;
        self.size = Size {
            width: width.min(self.max_side),
            height: height.min(self.max_side),
        };
        if self.can_draw() {
            self.target.configure(self.size.width, self.size.height);
        }
    }

    /// Draws a frame with `draw` into the surface's next texture and presents it. The frame is
    /// skipped when the window has no pixels, is hidden, or the texture timed out; a lost or
    /// outdated surface is configured again and the frame skipped, until an unbroken run of
    /// such frames lasts `MAX_TIME_WITHOUT_SURFACE`, which fails (a drawn, hidden, timed out
    /// or zero-size frame, or a resize, ends the run). A texture the GPU refuses, or an error
    /// of `draw`, fails with its cause; `draw`'s `RenderError::DeviceLost` tells the caller to
    /// open a new device.
    pub fn present(
        &mut self,
        draw: impl FnOnce(&wgpu::TextureView, Size) -> Result<(), RenderError>,
    ) -> Result<(), RuntimeError> {
        self.waited = Duration::ZERO;
        if !self.can_draw() {
            self.lost_since = None;
            return Ok(());
        }
        let asked = (self.clock)();
        let acquired = self.target.acquire();
        self.waited = (self.clock)().duration_since(asked);
        self.hidden = matches!(
            acquired,
            Acquired::Occluded | Acquired::Outdated | Acquired::Lost
        );
        let lost = match acquired {
            Acquired::Frame(frame, suboptimal) => {
                self.lost_since = None;
                draw(&frame.view, self.size)
                    .map_err(|error| RuntimeError::Gpu(error.to_string()))?;
                self.target.present(frame);
                if suboptimal {
                    self.target.configure(self.size.width, self.size.height);
                }
                return Ok(());
            }
            Acquired::Timeout | Acquired::Occluded => {
                self.lost_since = None;
                return Ok(());
            }
            Acquired::Validation => {
                return Err(RuntimeError::Gpu(
                    "the window's surface has no texture to draw into: the GPU refused it"
                        .to_string(),
                ));
            }
            Acquired::Outdated => "outdated",
            Acquired::Lost => "lost",
        };
        let now = (self.clock)();
        let since = *self.lost_since.get_or_insert(now);
        if now.duration_since(since) > MAX_TIME_WITHOUT_SURFACE {
            return Err(RuntimeError::Window(format!(
                "the window's surface stayed {lost} for {} seconds, though it was configured again at every frame",
                MAX_TIME_WITHOUT_SURFACE.as_secs()
            )));
        }
        self.target.configure(self.size.width, self.size.height);
        Ok(())
    }
}

/// A presenter and a renderer on the same device.
pub(crate) type Screen = (Presenter, Renderer);

/// Opens a screen on a new device.
pub(crate) type OpenScreen = Box<dyn FnMut() -> Result<Screen, RuntimeError>>;

/// Why a frame was not shown.
enum Failure {
    /// The device is lost, with its cause.
    Lost(String),
    Failed(RuntimeError),
}

impl From<RenderError> for Failure {
    fn from(error: RenderError) -> Failure {
        match error {
            RenderError::DeviceLost(cause) => Failure::Lost(cause),
            other => Failure::Failed(RuntimeError::Gpu(other.to_string())),
        }
    }
}

/// What the window draws with: a screen, the atlas it holds, and how to open a new screen
/// when the device is lost.
pub(crate) struct Display {
    /// `None` only while a new screen opens.
    screen: Option<Screen>,
    open: OpenScreen,
    /// The textures packed into `atlas`, by path, each the last version that packed: what a
    /// replacement that fails keeps.
    textures: BTreeMap<String, ProjectTexture>,
    atlas: Atlas,
    /// The window's size, which a new screen takes.
    size: (u32, u32),
    /// The renderer's frames are out of date: the screen is new, or frames came while
    /// nothing was drawn.
    stale: bool,
    /// What the caller has to tell the player, oldest first.
    warnings: Vec<String>,
}

impl Display {
    /// Opens a screen with `open` at `size`, and packs `textures` into its atlas, with a
    /// warning (see `take_warnings`) for each texture left out: it draws as a placeholder.
    pub(crate) fn new(
        mut open: OpenScreen,
        textures: &[ProjectTexture],
        size: (u32, u32),
    ) -> Result<Display, RuntimeError> {
        let (mut presenter, mut renderer) = open()?;
        presenter.resize(size.0, size.1);
        let (atlas, textures) = pack_textures(textures.to_vec(), renderer.max_page_side());
        let warnings = atlas.errors.iter().map(ToString::to_string).collect();
        renderer
            .set_atlas(&atlas)
            .map_err(|error| RuntimeError::Gpu(error.to_string()))?;
        Ok(Display {
            screen: Some((presenter, renderer)),
            open,
            textures,
            atlas,
            size,
            stale: true,
            warnings,
        })
    }

    /// Repacks the textures after they changed on disk: `update`'s textures replace those
    /// at the same path; a failed path (`update.failures`, or a new version that cannot be
    /// decoded or cut into frames) keeps its previous version; a path in neither list is
    /// dropped. The new atlas is uploaded, kept for a new screen, and drawn from the next
    /// frame. Returns a warning per failure (`<path>: <cause>, keeping the previous
    /// version`, or `; the texture is left out` without a previous version), and one per
    /// texture the atlas leaves out. Never touches the simulation.
    pub(crate) fn replace_textures(&mut self, update: &ProjectTextures) -> Vec<String> {
        let mut warnings = Vec::new();
        let mut next = Vec::new();
        for texture in &update.textures {
            let (_, path, png, meta) = texture;
            let previous = self
                .textures
                .get(path)
                .filter(|previous| *previous != texture);
            match previous.map(|previous| (previous, frames_of(png, meta))) {
                Some((previous, Err(error))) => {
                    warnings.push(format!("{path}: {error}, keeping the previous version"));
                    next.push(previous.clone());
                }
                _ => next.push(texture.clone()),
            }
        }
        for failure in &update.failures {
            match self.textures.get(&failure.path) {
                Some(previous) => {
                    warnings.push(format!("{failure}, keeping the previous version"));
                    next.push(previous.clone());
                }
                None => warnings.push(format!("{failure}; the texture is left out")),
            }
        }
        // The screen is missing only while `reopen` runs, which holds the display.
        let Some((_, renderer)) = &mut self.screen else {
            warnings.push("no GPU device to draw the changed textures with".to_string());
            return warnings;
        };
        let (atlas, textures) = pack_textures(next, renderer.max_page_side());
        warnings.extend(atlas.errors.iter().map(ToString::to_string));
        match renderer.set_atlas(&atlas) {
            // The next frame finds the loss and opens a new screen with this atlas.
            Ok(()) | Err(RenderError::DeviceLost(_)) => {}
            Err(error) => {
                warnings.push(format!(
                    "cannot upload the changed textures, the previous ones stay drawn: {error}"
                ));
                return warnings;
            }
        }
        self.textures = textures;
        self.atlas = atlas;
        self.stale = true;
        warnings
    }

    /// The warnings since the last call: textures left out of the atlas, a lost device.
    pub(crate) fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Follows the window to `width` by `height` physical pixels.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.size = (width, height);
        if let Some((presenter, _)) = &mut self.screen {
            presenter.resize(width, height);
        }
    }

    /// How long the last frame waited for the screen's texture (`Presenter::waited`).
    pub(crate) fn waited(&self) -> Duration {
        self.screen
            .as_ref()
            .map_or(Duration::ZERO, |(presenter, _)| presenter.waited())
    }

    /// Whether frames are drawn: not while the window has no pixels or is hidden.
    pub(crate) fn can_draw(&self) -> bool {
        let hidden = self
            .screen
            .as_ref()
            .is_some_and(|(presenter, _)| presenter.hidden());
        self.size.0 > 0 && self.size.1 > 0 && !hidden
    }

    /// Draws `current`, blended from `previous` by `alpha`, under `overlay`. The frames are
    /// uploaded when `new_frames` says they changed (or the renderer missed them). When the
    /// device is lost, a new screen opens with the atlas and the frame is drawn on it; a
    /// failure to open it, or its loss too, fails with both causes. Returns whether a frame
    /// was presented.
    pub(crate) fn show(
        &mut self,
        previous: Option<&RenderFrame>,
        current: &RenderFrame,
        overlay: &Overlay,
        new_frames: bool,
        alpha: f32,
    ) -> Result<bool, RuntimeError> {
        self.stale |= new_frames;
        let cause = match self.draw(previous, current, overlay, alpha) {
            Err(Failure::Lost(cause)) => cause,
            Err(Failure::Failed(error)) => return Err(error),
            Ok(presented) => return Ok(presented),
        };
        self.warnings.push(format!(
            "the GPU device was lost ({cause}); opening a new one"
        ));
        self.reopen().map_err(|error| {
            RuntimeError::Gpu(format!(
                "the GPU device was lost ({cause}), and a new one could not be opened: {error}"
            ))
        })?;
        match self.draw(previous, current, overlay, alpha) {
            Ok(presented) => Ok(presented),
            Err(Failure::Failed(error)) => Err(error),
            Err(Failure::Lost(again)) => Err(RuntimeError::Gpu(format!(
                "the GPU device was lost ({cause}), and the new one was lost too ({again})"
            ))),
        }
    }

    fn draw(
        &mut self,
        previous: Option<&RenderFrame>,
        current: &RenderFrame,
        overlay: &Overlay,
        alpha: f32,
    ) -> Result<bool, Failure> {
        let Some((presenter, renderer)) = &mut self.screen else {
            return Err(Failure::Failed(RuntimeError::Gpu(
                "no GPU device to draw with".to_string(),
            )));
        };
        if !presenter.can_draw() {
            return Ok(false);
        }
        // Asking the surface for a texture touches the device: a loss is caught first.
        renderer.check_device()?;
        if self.stale {
            renderer.set_frames(previous, current, overlay)?;
            self.stale = false;
        }
        let (mut drawn, mut lost) = (false, None);
        let presented = presenter.present(|view, size| {
            let result = renderer.draw(view, size, alpha, None);
            drawn = result.is_ok();
            if let Err(RenderError::DeviceLost(cause)) = &result {
                lost = Some(cause.clone());
            }
            result
        });
        if let Some(cause) = lost {
            return Err(Failure::Lost(cause));
        }
        match presented {
            Ok(()) => Ok(drawn),
            // A texture refused because the device was lost while it was asked for.
            Err(error) => match renderer.check_device() {
                Err(RenderError::DeviceLost(cause)) => Err(Failure::Lost(cause)),
                Ok(()) => Err(Failure::Failed(error)),
                // The GPU's own errors, often why the texture was refused, added to the
                // failure's message; the failure keeps its kind.
                Err(pending) => Err(Failure::Failed(match error {
                    RuntimeError::Gpu(message) => {
                        RuntimeError::Gpu(format!("{message}; {pending}"))
                    }
                    RuntimeError::Window(message) => {
                        RuntimeError::Window(format!("{message}; {pending}"))
                    }
                    other => other,
                })),
            },
        }
    }

    /// Replaces the screen with one on a new device, holding the atlas, at the window's size.
    fn reopen(&mut self) -> Result<(), RuntimeError> {
        // The old surface goes first: a window holds one surface at a time.
        self.screen = None;
        let (mut presenter, mut renderer) = (self.open)()?;
        presenter.resize(self.size.0, self.size.1);
        renderer
            .set_atlas(&self.atlas)
            .map_err(|error| RuntimeError::Gpu(error.to_string()))?;
        self.screen = Some((presenter, renderer));
        self.stale = true;
        Ok(())
    }
}

/// Packs `textures` into pages of at most `max_page_side`, with the textures that packed, by
/// path.
fn pack_textures(
    textures: Vec<ProjectTexture>,
    max_page_side: u32,
) -> (Atlas, BTreeMap<String, ProjectTexture>) {
    let sources: Vec<TextureSource<'_>> = textures
        .iter()
        .map(|(id, path, png, meta)| TextureSource {
            id: *id,
            path: Some(path),
            png,
            meta,
        })
        .collect();
    let atlas = pack(&sources, max_page_side);
    let packed = textures
        .into_iter()
        .filter(|(_, path, ..)| !atlas.errors.iter().any(|error| error.path() == Some(path)))
        .map(|texture| (texture.1.clone(), texture))
        .collect();
    (atlas, packed)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use ludenea_app::players::{LiveDevices, automatic_claims};
    use ludenea_app::project::TextureFailure;
    use ludenea_render::gpu::headless;

    use super::*;
    use crate::{GameFiles, Runner};

    /// What the fake surface was asked, and the outcomes it gives, in order (a frame once
    /// they run out).
    struct Log {
        outcomes: VecDeque<Outcome>,
        acquired: u32,
        configured: Vec<(u32, u32)>,
        presented: u32,
        /// The presenter's clock.
        now: Instant,
        /// How far the clock moves while a texture is asked for.
        acquire_wait: Duration,
    }

    impl Default for Log {
        fn default() -> Log {
            Log {
                outcomes: VecDeque::new(),
                acquired: 0,
                configured: Vec::new(),
                presented: 0,
                now: now(),
                acquire_wait: Duration::ZERO,
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Outcome {
        Frame,
        Timeout,
        Occluded,
        Outdated,
        Lost,
        Validation,
        /// The device is lost while the texture is asked for, which the GPU then refuses.
        DeviceLost,
        /// The device raises an error while the texture is asked for, which it then refuses.
        GpuError,
        /// The device raises an error while the texture is asked for, and the surface is lost.
        LostAfterGpuError,
    }

    /// An offscreen stand-in for a window's surface.
    struct Fake {
        log: Rc<RefCell<Log>>,
        device: wgpu::Device,
        texture: wgpu::Texture,
    }

    impl Target for Fake {
        fn configure(&mut self, width: u32, height: u32) {
            self.log.borrow_mut().configured.push((width, height));
        }

        fn acquire(&mut self) -> Acquired {
            let mut log = self.log.borrow_mut();
            log.acquired += 1;
            let wait = log.acquire_wait;
            log.now += wait;
            match log.outcomes.pop_front().unwrap_or(Outcome::Frame) {
                Outcome::Frame => Acquired::Frame(
                    Frame {
                        view: self.texture.create_view(&Default::default()),
                        texture: None,
                    },
                    false,
                ),
                Outcome::Timeout => Acquired::Timeout,
                Outcome::Occluded => Acquired::Occluded,
                Outcome::Outdated => Acquired::Outdated,
                Outcome::Lost => Acquired::Lost,
                Outcome::Validation => Acquired::Validation,
                Outcome::DeviceLost => {
                    lose(&self.device);
                    Acquired::Validation
                }
                Outcome::GpuError => {
                    raise_error(&self.device);
                    Acquired::Validation
                }
                Outcome::LostAfterGpuError => {
                    raise_error(&self.device);
                    Acquired::Lost
                }
            }
        }

        fn present(&mut self, _frame: Frame) {
            self.log.borrow_mut().presented += 1;
        }
    }

    /// Makes `device` raise a validation error: a buffer both read and written by the host
    /// is invalid.
    fn raise_error(device: &wgpu::Device) {
        let _invalid = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("invalid"),
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::MAP_WRITE,
            mapped_at_creation: false,
        });
    }

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    fn device() -> (wgpu::Device, wgpu::Queue) {
        pollster::block_on(headless()).expect("a GPU adapter, hardware or software")
    }

    fn fake(device: &wgpu::Device, log: &Rc<RefCell<Log>>) -> Presenter {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fake surface"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let target = Fake {
            log: Rc::clone(log),
            device: device.clone(),
            texture,
        };
        let clock = Rc::clone(log);
        let mut presenter = Presenter::with_target(
            Box::new(target),
            FORMAT,
            4096,
            Box::new(move || clock.borrow().now),
        );
        presenter.resize(64, 64);
        presenter
    }

    /// Loses `device` as a driver reset would. wgpu reports a destroyed device lost once its
    /// submitted work has retired, so the work is waited for first.
    fn lose(device: &wgpu::Device) {
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the device's work finishes");
        device.destroy();
    }

    fn presenter(outcomes: &[Outcome]) -> (Presenter, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log {
            outcomes: outcomes.iter().copied().collect(),
            ..Log::default()
        }));
        let (device, _queue) = device();
        (fake(&device, &log), log)
    }

    /// Presents a frame and tells whether it was drawn.
    fn drawn(presenter: &mut Presenter) -> Result<bool, RuntimeError> {
        let mut drawn = false;
        presenter.present(|_, size| {
            assert_eq!(
                size,
                Size {
                    width: 64,
                    height: 64
                }
            );
            drawn = true;
            Ok(())
        })?;
        Ok(drawn)
    }

    fn empty_game() -> Runner {
        let files = GameFiles {
            scene: "Scene(version: 2, next_id: 0, resources: {}, entities: [])".to_string(),
            assets: Vec::new(),
            edits: None,
        };
        let mut runner = Runner::load(swarm::app, &files, None, Vec::new()).unwrap();
        runner.start(automatic_claims(1)).unwrap();
        runner
    }

    /// A display whose every opening makes a new device behind a fake surface, the devices
    /// listed in `opened`; `lose_new` destroys each new device as it opens.
    fn display(
        log: &Rc<RefCell<Log>>,
        opened: &Rc<RefCell<Vec<wgpu::Device>>>,
        lose_new: &Rc<RefCell<bool>>,
    ) -> Display {
        let (log, opened, lose_new) = (Rc::clone(log), Rc::clone(opened), Rc::clone(lose_new));
        let open = move || {
            let (device, queue) = device();
            let presenter = fake(&device, &log);
            let renderer = Renderer::new(device.clone(), queue, FORMAT);
            if *lose_new.borrow() {
                lose(&device);
            }
            opened.borrow_mut().push(device);
            Ok((presenter, renderer))
        };
        Display::new(Box::new(open), &[], (64, 64)).unwrap()
    }

    #[test]
    fn textures_left_out_of_the_atlas_and_a_lost_device_are_warnings() {
        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let (log2, opened2) = (Rc::clone(&log), Rc::clone(&opened));
        let open = move || {
            let (device, queue) = device();
            let presenter = fake(&device, &log2);
            let renderer = Renderer::new(device.clone(), queue, FORMAT);
            opened2.borrow_mut().push(device);
            Ok((presenter, renderer))
        };
        let broken: ProjectTexture = (
            ludenea_ecs::AssetId(7),
            "broken.png".to_string(),
            b"not a png".to_vec(),
            Default::default(),
        );
        let mut display = Display::new(Box::new(open), &[broken], (64, 64)).unwrap();
        let [warning] = display.take_warnings().try_into().unwrap();
        assert!(warning.contains("broken.png"), "{warning}");
        assert!(display.take_warnings().is_empty(), "taken once");
        let runner = empty_game();
        let (previous, current) = runner.frames();
        lose(&opened.borrow()[0]);
        display
            .show(previous, current.unwrap(), &Overlay::default(), true, 1.0)
            .unwrap();
        let [warning] = display.take_warnings().try_into().unwrap();
        assert!(warning.starts_with("the GPU device was lost"), "{warning}");
    }

    #[test]
    fn the_wait_for_a_texture_is_measured_per_frame() {
        let (mut presenter, log) = presenter(&[Outcome::Frame, Outcome::Timeout]);
        log.borrow_mut().acquire_wait = Duration::from_millis(5);
        assert!(drawn(&mut presenter).unwrap());
        assert_eq!(presenter.waited(), Duration::from_millis(5));
        assert!(!drawn(&mut presenter).unwrap(), "timed out");
        assert_eq!(presenter.waited(), Duration::from_millis(5));
        presenter.resize(0, 0);
        assert!(!drawn(&mut presenter).unwrap());
        assert_eq!(presenter.waited(), Duration::ZERO, "nothing is asked for");
    }

    #[test]
    fn zero_size_skips_drawing_and_keeps_ticking() {
        let (mut presenter, log) = presenter(&[]);
        presenter.resize(0, 0);
        assert!(!drawn(&mut presenter).unwrap());
        assert_eq!(log.borrow().acquired, 0, "no texture is asked at zero size");

        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = display(&log, &opened, &Rc::new(RefCell::new(false)));
        let mut runner = empty_game();
        assert!(display.can_draw());
        display.resize(0, 0);
        assert!(!display.can_draw(), "the loop paces itself at zero size");
        for _ in 0..10 {
            let update = runner
                .frame(1.0 / 60.0 + 1e-9, &LiveDevices::default())
                .unwrap();
            let (previous, current) = runner.frames();
            display
                .show(
                    previous,
                    current.unwrap(),
                    &Overlay::default(),
                    update.new_frames,
                    update.alpha,
                )
                .unwrap();
        }
        assert_eq!(runner.tick(), 10, "the simulation keeps its pace");
        assert_eq!(log.borrow().acquired, 0, "nothing is drawn at zero size");
        display.resize(64, 64);
        let (previous, current) = runner.frames();
        display
            .show(previous, current.unwrap(), &Overlay::default(), false, 0.5)
            .unwrap();
        assert_eq!(log.borrow().presented, 1, "drawing resumes at a size");
    }

    #[test]
    fn lost_surface_is_configured_again() {
        for lost in [Outcome::Lost, Outcome::Outdated] {
            let (mut presenter, log) = presenter(&[lost]);
            let configured = log.borrow().configured.len();
            assert!(!drawn(&mut presenter).unwrap(), "{lost:?} skips the frame");
            assert_eq!(
                log.borrow().configured[configured..],
                [(64, 64)],
                "{lost:?} configures the surface again"
            );
            assert!(drawn(&mut presenter).unwrap(), "the next frame draws");
            assert_eq!(log.borrow().presented, 1);
        }
    }

    #[test]
    fn a_surface_outdated_for_many_frames_fails_only_after_five_seconds() {
        let (mut presenter, log) = presenter(&[Outcome::Outdated; 1001]);
        for _ in 0..1000 {
            assert!(!drawn(&mut presenter).unwrap(), "an interactive resize");
            assert!(presenter.hidden(), "the caller paces the frames meanwhile");
        }
        log.borrow_mut().now += MAX_TIME_WITHOUT_SURFACE + Duration::from_millis(1);
        let error = drawn(&mut presenter).unwrap_err();
        assert!(
            matches!(&error, RuntimeError::Window(message) if message.contains("outdated")),
            "{error}"
        );
    }

    #[test]
    fn a_frame_drawn_restarts_the_time_without_surface() {
        let (mut presenter, log) = presenter(&[Outcome::Lost, Outcome::Frame, Outcome::Lost]);
        assert!(!drawn(&mut presenter).unwrap());
        log.borrow_mut().now += MAX_TIME_WITHOUT_SURFACE;
        assert!(drawn(&mut presenter).unwrap());
        log.borrow_mut().now += MAX_TIME_WITHOUT_SURFACE;
        assert!(!drawn(&mut presenter).unwrap(), "a new run of lost frames");
    }

    /// Something that happens to the window between two lost frames.
    type Interrupt = fn(&mut Presenter, &Rc<RefCell<Log>>);

    #[test]
    fn only_an_unbroken_run_of_lost_frames_counts() {
        let interruptions: [(&str, Interrupt); 4] = [
            ("occluded", |presenter, log| {
                log.borrow_mut().outcomes.push_back(Outcome::Occluded);
                assert!(!drawn(presenter).unwrap());
            }),
            ("timed out", |presenter, log| {
                log.borrow_mut().outcomes.push_back(Outcome::Timeout);
                assert!(!drawn(presenter).unwrap());
            }),
            ("resized", |presenter, _| presenter.resize(64, 64)),
            ("minimized", |presenter, _| {
                presenter.resize(0, 0);
                assert!(!drawn(presenter).unwrap());
                presenter.resize(64, 64);
            }),
        ];
        for (name, interrupt) in interruptions {
            let (mut presenter, log) = presenter(&[Outcome::Lost]);
            assert!(!drawn(&mut presenter).unwrap());
            interrupt(&mut presenter, &log);
            log.borrow_mut().now += MAX_TIME_WITHOUT_SURFACE + Duration::from_secs(1);
            log.borrow_mut().outcomes.push_back(Outcome::Outdated);
            assert!(!drawn(&mut presenter).unwrap(), "{name}: a new run starts");
        }
    }

    #[test]
    fn a_refused_texture_names_the_gpu_errors_behind_it() {
        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = display(&log, &opened, &Rc::new(RefCell::new(false)));
        let runner = empty_game();
        let (previous, current) = runner.frames();
        log.borrow_mut().outcomes.push_back(Outcome::GpuError);
        let error = display
            .show(previous, current.unwrap(), &Overlay::default(), true, 1.0)
            .unwrap_err();
        let RuntimeError::Gpu(message) = &error else {
            panic!("{error}");
        };
        assert!(message.contains("refused"), "{message}");
        assert!(message.contains("earlier GPU work"), "{message}");
    }

    #[test]
    fn a_surface_lost_for_too_long_stays_a_window_failure_beside_gpu_errors() {
        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = display(&log, &opened, &Rc::new(RefCell::new(false)));
        let runner = empty_game();
        let (previous, current) = runner.frames();
        let show = |display: &mut Display| {
            display.show(previous, current.unwrap(), &Overlay::default(), true, 1.0)
        };
        log.borrow_mut().outcomes.push_back(Outcome::Lost);
        assert!(!show(&mut display).unwrap());
        log.borrow_mut().now += MAX_TIME_WITHOUT_SURFACE + Duration::from_millis(1);
        log.borrow_mut()
            .outcomes
            .push_back(Outcome::LostAfterGpuError);
        let error = show(&mut display).unwrap_err();
        let RuntimeError::Window(message) = &error else {
            panic!("a window failure, not {error:?}");
        };
        assert!(message.contains("stayed lost"), "{message}");
        assert!(message.contains("earlier GPU work"), "{message}");
    }

    #[test]
    fn timeout_skips_one_frame() {
        let (mut presenter, log) = presenter(&[Outcome::Timeout]);
        let configured = log.borrow().configured.len();
        assert!(!drawn(&mut presenter).unwrap());
        assert!(drawn(&mut presenter).unwrap());
        assert_eq!(
            log.borrow().configured.len(),
            configured,
            "nothing to configure"
        );
        assert_eq!((log.borrow().acquired, log.borrow().presented), (2, 1));
    }

    #[test]
    fn a_hidden_window_draws_again_once_it_shows() {
        let (mut presenter, log) = presenter(&[Outcome::Occluded, Outcome::Occluded]);
        assert!(!presenter.hidden());
        assert!(!drawn(&mut presenter).unwrap());
        assert!(presenter.hidden(), "the loop stops waiting on the display");
        assert!(!drawn(&mut presenter).unwrap());
        assert!(
            drawn(&mut presenter).unwrap(),
            "asked again, it draws once shown"
        );
        assert!(!presenter.hidden());
        assert_eq!((log.borrow().acquired, log.borrow().presented), (3, 1));
    }

    #[test]
    fn a_surface_the_gpu_refuses_fails_with_the_cause() {
        let (mut presenter, _log) = presenter(&[Outcome::Validation]);
        let error = drawn(&mut presenter).unwrap_err();
        assert!(matches!(error, RuntimeError::Gpu(_)), "{error}");
    }

    #[test]
    fn a_device_lost_between_ticks_rebuilds() {
        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = display(&log, &opened, &Rc::new(RefCell::new(false)));
        let runner = empty_game();
        let (previous, current) = runner.frames();
        let current = current.unwrap();
        display
            .show(previous, current, &Overlay::default(), true, 1.0)
            .unwrap();
        lose(&opened.borrow()[0]);
        // No new tick, so no upload, and the lost device's surface answers `Lost`: the loss
        // must be found before the texture is asked for.
        log.borrow_mut().outcomes.push_back(Outcome::Lost);
        display
            .show(previous, current, &Overlay::default(), false, 0.5)
            .unwrap();
        assert_eq!(opened.borrow().len(), 2, "a new device opened");
        assert!(
            display
                .show(previous, current, &Overlay::default(), false, 0.5)
                .unwrap()
        );
        assert_eq!(log.borrow().presented, 2);
    }

    #[test]
    fn a_device_lost_while_asking_for_a_texture_rebuilds() {
        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = display(&log, &opened, &Rc::new(RefCell::new(false)));
        let runner = empty_game();
        let (previous, current) = runner.frames();
        let current = current.unwrap();
        display
            .show(previous, current, &Overlay::default(), true, 1.0)
            .unwrap();
        log.borrow_mut().outcomes.push_back(Outcome::DeviceLost);
        assert!(
            display
                .show(previous, current, &Overlay::default(), false, 0.5)
                .unwrap()
        );
        assert_eq!((opened.borrow().len(), log.borrow().presented), (2, 2));
    }

    #[test]
    fn lost_device_rebuilds_once_then_fails_with_the_cause() {
        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let lose_new = Rc::new(RefCell::new(false));
        let mut display = display(&log, &opened, &lose_new);
        let runner = empty_game();
        let (previous, current) = runner.frames();
        let current = current.unwrap();
        let show =
            |display: &mut Display| display.show(previous, current, &Overlay::default(), true, 1.0);
        show(&mut display).unwrap();
        assert_eq!((opened.borrow().len(), log.borrow().presented), (1, 1));

        // The device is lost: a new one opens and the frame is drawn on it.
        lose(&opened.borrow()[0]);
        show(&mut display).unwrap();
        assert_eq!((opened.borrow().len(), log.borrow().presented), (2, 2));

        // Lost again, and the new device is lost as it opens: one attempt, then the cause.
        *lose_new.borrow_mut() = true;
        lose(&opened.borrow()[1]);
        let error = show(&mut display).unwrap_err();
        assert_eq!(opened.borrow().len(), 3, "a single rebuild per loss");
        assert_eq!(log.borrow().presented, 2);
        assert!(
            matches!(&error, RuntimeError::Gpu(message) if message.contains("lost")),
            "{error}"
        );
    }

    /// A camera showing 4 by 4 units around the origin, filled by a sprite of texture 1 at a
    /// pixel per unit.
    const STILL: &str = r#"Scene(version: 2, next_id: 2, resources: {}, entities: [
        (id: 0, components: {"ludenea::Pose": (position: (x: 0.0, y: 0.0), rotation: (cos: 1.0, sin: 0.0)), "ludenea::Camera2d": (view: (x: 4.0, y: 4.0))}),
        (id: 1, components: {"ludenea::Pose": (position: (x: 0.0, y: 0.0), rotation: (cos: 1.0, sin: 0.0)), "ludenea::Sprite": (texture: "00000000000000000000000000000001", frame: 0, layer: "world", order: 0, flip_x: false, flip_y: false, tint: (4294967295))}),
    ])"#;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const MAGENTA: [u8; 4] = [255, 0, 255, 255];

    /// A 4 by 4 PNG of one colour.
    fn solid(color: [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 4, 4);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&color.repeat(16)).unwrap();
        writer.finish().unwrap();
        bytes
    }

    /// Texture 1 at `hero.png`, a pixel per unit, nearest filtering.
    fn hero(png: Vec<u8>) -> ProjectTexture {
        let meta = ludenea_app::meta::TextureMeta {
            pixels_per_unit: 1,
            filter: ludenea_app::meta::Filter::Nearest,
            ..Default::default()
        };
        (ludenea_ecs::AssetId(1), "hero.png".to_string(), png, meta)
    }

    fn still_game() -> Runner {
        let files = GameFiles {
            scene: STILL.to_string(),
            assets: Vec::new(),
            edits: None,
        };
        let mut runner = Runner::load(swarm::app, &files, None, Vec::new()).unwrap();
        runner.start(automatic_claims(1)).unwrap();
        runner
    }

    /// A display of `textures` on a new device per opening, the devices listed in `opened`.
    fn textured(textures: &[ProjectTexture], opened: &Rc<RefCell<Vec<wgpu::Device>>>) -> Display {
        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::clone(opened);
        let open = move || {
            let (device, queue) = device();
            let presenter = fake(&device, &log);
            let renderer = Renderer::new(device.clone(), queue, FORMAT);
            opened.borrow_mut().push(device);
            Ok((presenter, renderer))
        };
        Display::new(Box::new(open), textures, (64, 64)).unwrap()
    }

    /// Shows the game's frame on `display`, then reads the pixel at the centre of what the
    /// display's renderer draws.
    fn centre(display: &mut Display, runner: &Runner) -> [u8; 4] {
        let (previous, current) = runner.frames();
        display
            .show(previous, current.unwrap(), &Overlay::default(), false, 1.0)
            .unwrap();
        let (_, renderer) = display.screen.as_mut().unwrap();
        let size = Size {
            width: 64,
            height: 64,
        };
        let rgba = renderer.render_rgba(size, 1.0, None).unwrap();
        let at = (32 * 64 + 32) * 4;
        rgba[at..at + 4].try_into().unwrap()
    }

    fn update(textures: Vec<ProjectTexture>, failures: Vec<TextureFailure>) -> ProjectTextures {
        ProjectTextures { textures, failures }
    }

    #[test]
    fn replaced_texture_changes_the_drawn_pixels() {
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = textured(&[hero(solid(RED))], &opened);
        let runner = still_game();
        assert_eq!(centre(&mut display, &runner), RED);
        let warnings = display.replace_textures(&update(vec![hero(solid(BLUE))], Vec::new()));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(centre(&mut display, &runner), BLUE);
    }

    #[test]
    fn broken_replacement_keeps_the_previous_texture() {
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = textured(&[hero(solid(RED))], &opened);
        let runner = still_game();
        assert_eq!(centre(&mut display, &runner), RED);
        let truncated = solid(BLUE)[..20].to_vec();
        let warnings = display.replace_textures(&update(vec![hero(truncated)], Vec::new()));
        let [warning] = warnings.as_slice() else {
            panic!("{warnings:?}")
        };
        assert!(
            warning.starts_with("hero.png: cannot decode the PNG: "),
            "{warning}"
        );
        assert!(
            warning.ends_with(", keeping the previous version"),
            "{warning}"
        );
        assert_eq!(centre(&mut display, &runner), RED);
        // Fixed, the new version is drawn.
        let warnings = display.replace_textures(&update(vec![hero(solid(BLUE))], Vec::new()));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(centre(&mut display, &runner), BLUE);
    }

    #[test]
    fn invalid_meta_keeps_the_previous_texture() {
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = textured(&[hero(solid(RED))], &opened);
        let runner = still_game();
        let failure = TextureFailure {
            path: "hero.png".to_string(),
            cause: "cannot read its meta: expected `Meta`".to_string(),
        };
        let warnings = display.replace_textures(&update(Vec::new(), vec![failure]));
        assert_eq!(
            warnings,
            ["hero.png: cannot read its meta: expected `Meta`, keeping the previous version"]
        );
        assert_eq!(centre(&mut display, &runner), RED);
    }

    #[test]
    fn a_texture_gone_from_the_project_is_dropped() {
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = textured(&[hero(solid(RED))], &opened);
        let runner = still_game();
        let warnings = display.replace_textures(&update(Vec::new(), Vec::new()));
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(centre(&mut display, &runner), MAGENTA, "a placeholder");
        // A failure with no version to keep leaves the texture out.
        let failure = TextureFailure {
            path: "hero.png".to_string(),
            cause: "cannot read the file: denied".to_string(),
        };
        let warnings = display.replace_textures(&update(Vec::new(), vec![failure]));
        assert_eq!(
            warnings,
            ["hero.png: cannot read the file: denied; the texture is left out"]
        );
        assert_eq!(centre(&mut display, &runner), MAGENTA);
    }

    #[test]
    fn replaced_textures_survive_a_reopen() {
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = textured(&[hero(solid(RED))], &opened);
        let runner = still_game();
        assert_eq!(centre(&mut display, &runner), RED);
        display.replace_textures(&update(vec![hero(solid(BLUE))], Vec::new()));
        lose(&opened.borrow()[0]);
        assert_eq!(centre(&mut display, &runner), BLUE);
        assert_eq!(opened.borrow().len(), 2, "a new device opened");
    }

    /// A camera showing 4 by 4 units around the origin and one sprite circling it, a radius
    /// of 1, a quarter turn per tick from the second tick on.
    const CIRCLING: &str = r#"Scene(version: 2, next_id: 2, resources: {}, entities: [
        (id: 0, components: {"ludenea::Pose": (position: (x: 0.0, y: 0.0), rotation: (cos: 1.0, sin: 0.0)), "ludenea::Camera2d": (view: (x: 4.0, y: 4.0))}),
        (id: 1, components: {"ludenea::Pose": (position: (x: 1.0, y: 0.0), rotation: (cos: 1.0, sin: 0.0)), "ludenea::Sprite": (texture: "00000000000000000000000000000001", frame: 0, layer: "world", order: 0, flip_x: false, flip_y: false, tint: (4294967295)), "swarm::Wanderer": (center: (x: 0.0, y: 0.0), radius: 1.0, phase: 0.0, speed: 1.5707964)}),
    ])"#;

    #[test]
    fn a_frame_that_runs_two_ticks_draws_the_newest_tick_unblended() {
        let files = GameFiles {
            scene: CIRCLING.to_string(),
            assets: Vec::new(),
            edits: None,
        };
        let mut runner = Runner::load(swarm::app, &files, None, Vec::new()).unwrap();
        runner.start(automatic_claims(1)).unwrap();
        let (device, queue) = device();
        let mut renderer = Renderer::new(device, queue, FORMAT);
        let size = Size {
            width: 64,
            height: 64,
        };
        let mut halfway = |runner: &Runner, previous: bool| {
            let (before, current) = runner.frames();
            let before = before.filter(|_| previous);
            renderer
                .set_frames(before, current.unwrap(), &Overlay::default())
                .unwrap();
            renderer.render_rgba(size, 0.5, None).unwrap()
        };
        // The first tick puts the sprite where the scene has it; from then on it moves.
        let one_tick = 1.0 / 60.0 + 1e-9;
        runner.frame(one_tick, &LiveDevices::default()).unwrap();
        // One tick: blended halfway between the two ticks.
        let update = runner.frame(one_tick, &LiveDevices::default()).unwrap();
        assert_eq!(update.tick, 2);
        assert!(halfway(&runner, true) != halfway(&runner, false), "blended");
        // Two ticks in one frame: the frame kept from before is two ticks old, so the newest
        // tick is drawn as is rather than blended across two ticks with a one-tick alpha.
        let update = runner
            .frame(2.0 / 60.0 + 1e-9, &LiveDevices::default())
            .unwrap();
        assert_eq!(update.tick, 4);
        assert!(
            halfway(&runner, true) == halfway(&runner, false),
            "not blended"
        );
    }
}
