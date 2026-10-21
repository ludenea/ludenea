//! The window's surface and what draws into it. `Presenter` turns the outcomes of asking the
//! surface for a texture into actions: a frame drawn and presented, a frame skipped (a
//! zero-size or hidden window, a timeout), the surface configured again (lost or outdated),
//! or a failure with its cause. `Display` draws the match through the presenter with a
//! renderer, and replaces both on a new device once when the device is lost.

use ludenea_app::project::ProjectTexture;
use ludenea_draw::frame::RenderFrame;
use ludenea_render::atlas::{Atlas, TextureSource, pack};
use ludenea_render::gpu::{Overlay, RenderError, Renderer, Size};

use crate::RuntimeError;

/// Frames in a row the surface may be lost or outdated, each time configured again, before
/// the window gives up on it: about a second at 60 frames per second.
const MAX_FRAMES_WITHOUT_SURFACE: u32 = 60;

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
    /// Frames in a row the surface was lost or outdated.
    without_surface: u32,
    /// The last texture asked for was refused because the window is hidden.
    hidden: bool,
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
        Ok(Presenter::with_target(Box::new(target), format, max_side))
    }

    /// A presenter of zero size on `target`.
    fn with_target(
        target: Box<dyn Target>,
        format: wgpu::TextureFormat,
        max_side: u32,
    ) -> Presenter {
        Presenter {
            target,
            format,
            size: Size {
                width: 0,
                height: 0,
            },
            max_side,
            without_surface: 0,
            hidden: false,
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

    /// Whether the window was hidden (behind another window, on another desktop, or the
    /// screen locked) when a frame was last presented: the display no longer paces the loop,
    /// though `present` keeps asking, and draws once the window shows.
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// Follows the window to `width` by `height` physical pixels, kept within the device's
    /// texture limit. At a side of zero, frames are skipped until the next resize.
    pub fn resize(&mut self, width: u32, height: u32) {
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
    /// outdated surface is configured again and the frame skipped, until it stays so for
    /// `MAX_FRAMES_WITHOUT_SURFACE` frames, which fails. A texture the GPU refuses, or an
    /// error of `draw`, fails with its cause; `draw`'s `RenderError::DeviceLost` tells the
    /// caller to open a new device.
    pub fn present(
        &mut self,
        draw: impl FnOnce(&wgpu::TextureView, Size) -> Result<(), RenderError>,
    ) -> Result<(), RuntimeError> {
        if !self.can_draw() {
            return Ok(());
        }
        let acquired = self.target.acquire();
        self.hidden = matches!(acquired, Acquired::Occluded);
        let lost = match acquired {
            Acquired::Frame(frame, suboptimal) => {
                self.without_surface = 0;
                draw(&frame.view, self.size)
                    .map_err(|error| RuntimeError::Gpu(error.to_string()))?;
                self.target.present(frame);
                if suboptimal {
                    self.target.configure(self.size.width, self.size.height);
                }
                return Ok(());
            }
            Acquired::Timeout | Acquired::Occluded => return Ok(()),
            Acquired::Validation => {
                return Err(RuntimeError::Gpu(
                    "the window's surface has no texture to draw into: the GPU refused it"
                        .to_string(),
                ));
            }
            Acquired::Outdated => "outdated",
            Acquired::Lost => "lost",
        };
        self.without_surface += 1;
        if self.without_surface > MAX_FRAMES_WITHOUT_SURFACE {
            return Err(RuntimeError::Window(format!(
                "the window's surface stayed {lost} for {MAX_FRAMES_WITHOUT_SURFACE} frames, though it was configured again each time"
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
    atlas: Atlas,
    /// The window's size, which a new screen takes.
    size: (u32, u32),
    /// The renderer's frames are out of date: the screen is new, or frames came while
    /// nothing was drawn.
    stale: bool,
}

impl Display {
    /// Opens a screen with `open` at `size`, and packs `textures` into its atlas, printing a
    /// warning for each texture left out (it draws as a placeholder).
    pub(crate) fn new(
        mut open: OpenScreen,
        textures: &[ProjectTexture],
        size: (u32, u32),
    ) -> Result<Display, RuntimeError> {
        let (mut presenter, mut renderer) = open()?;
        presenter.resize(size.0, size.1);
        let sources: Vec<TextureSource<'_>> = textures
            .iter()
            .map(|(id, path, png, meta)| TextureSource {
                id: *id,
                path: Some(path),
                png,
                meta,
            })
            .collect();
        let atlas = pack(&sources, renderer.max_page_side());
        for error in &atlas.errors {
            eprintln!("warning: {error}");
        }
        renderer
            .set_atlas(&atlas)
            .map_err(|error| RuntimeError::Gpu(error.to_string()))?;
        Ok(Display {
            screen: Some((presenter, renderer)),
            open,
            atlas,
            size,
            stale: true,
        })
    }

    /// Follows the window to `width` by `height` physical pixels.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.size = (width, height);
        if let Some((presenter, _)) = &mut self.screen {
            presenter.resize(width, height);
        }
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
        eprintln!("warning: the GPU device was lost ({cause}); opening a new one");
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
        match lost {
            Some(cause) => Err(Failure::Lost(cause)),
            None => presented.map(|()| drawn).map_err(Failure::Failed),
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use ludenea_app::players::{LiveDevices, automatic_claims};
    use ludenea_render::gpu::headless;

    use super::*;
    use crate::{GameFiles, Runner};

    /// What the fake surface was asked, and the outcomes it gives, in order (a frame once
    /// they run out).
    #[derive(Default)]
    struct Log {
        outcomes: VecDeque<Outcome>,
        acquired: u32,
        configured: Vec<(u32, u32)>,
        presented: u32,
    }

    #[derive(Clone, Copy, Debug)]
    enum Outcome {
        Frame,
        Timeout,
        Occluded,
        Outdated,
        Lost,
        Validation,
    }

    /// An offscreen stand-in for a window's surface.
    struct Fake {
        log: Rc<RefCell<Log>>,
        texture: wgpu::Texture,
    }

    impl Target for Fake {
        fn configure(&mut self, width: u32, height: u32) {
            self.log.borrow_mut().configured.push((width, height));
        }

        fn acquire(&mut self) -> Acquired {
            let mut log = self.log.borrow_mut();
            log.acquired += 1;
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
            }
        }

        fn present(&mut self, _frame: Frame) {
            self.log.borrow_mut().presented += 1;
        }
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
            texture,
        };
        let mut presenter = Presenter::with_target(Box::new(target), FORMAT, 4096);
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
    fn zero_size_skips_drawing_and_keeps_ticking() {
        let (mut presenter, log) = presenter(&[]);
        presenter.resize(0, 0);
        assert!(!drawn(&mut presenter).unwrap());
        assert_eq!(log.borrow().acquired, 0, "no texture is asked at zero size");

        let log = Rc::new(RefCell::new(Log::default()));
        let opened = Rc::new(RefCell::new(Vec::new()));
        let mut display = display(&log, &opened, &Rc::new(RefCell::new(false)));
        let mut runner = empty_game();
        display.resize(0, 0);
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
    fn a_surface_that_stays_lost_fails_naming_it() {
        let (mut presenter, _log) =
            presenter(&[Outcome::Lost; MAX_FRAMES_WITHOUT_SURFACE as usize + 1]);
        for _ in 0..MAX_FRAMES_WITHOUT_SURFACE {
            assert!(!drawn(&mut presenter).unwrap());
        }
        let error = drawn(&mut presenter).unwrap_err();
        assert!(
            matches!(&error, RuntimeError::Window(message) if message.contains("lost")),
            "{error}"
        );
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
}
