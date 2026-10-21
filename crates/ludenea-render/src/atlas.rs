//! PNG decoding and atlas packing: textures are decoded, cut into frames, and packed into
//! pages the renderer uploads. GPU-free and byte-for-byte reproducible: the same sources, in
//! any order, give the same pages.

use std::collections::BTreeMap;
use std::io::Cursor;

use ludenea_app::meta::{Filter, Grid, TextureMeta};
use ludenea_ecs::AssetId;
use serde::{Deserialize, Serialize};

/// How many bytes one decoded PNG may take, checked against its declared dimensions before
/// the pixel buffer is allocated (the decoder's own limit only covers its internal buffers):
/// room for a 4096 by 4096 RGBA image, while a file declaring absurd dimensions is refused.
const DECODE_LIMIT_BYTES: usize = 256 * 1024 * 1024;

/// One texture to pack: its PNG bytes and how its meta cuts them into frames.
#[derive(Clone, Copy, Debug)]
pub struct TextureSource<'a> {
    pub id: AssetId,
    /// The project path, named by errors when known.
    pub path: Option<&'a str>,
    pub png: &'a [u8],
    pub meta: &'a TextureMeta,
}

/// The packed pages, where each frame `(texture, frame)` lies in them, and the textures that
/// could not be packed, sorted by identifier.
#[derive(Clone, Debug, PartialEq)]
pub struct Atlas {
    pub pages: Vec<AtlasPage>,
    pub regions: BTreeMap<(AssetId, u32), Region>,
    pub errors: Vec<TextureError>,
}

/// One page of RGBA pixels, row by row from the top-left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlasPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A frame's rectangle in its page, without the extrusion around it, and how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub page: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// In pixels from the frame's top-left corner.
    pub pivot: (u32, u32),
    pub pixels_per_unit: u32,
    pub filter: Filter,
}

/// Why a texture was not packed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextureError {
    #[error("{}: cannot decode the PNG: {reason}", named(*id, path))]
    Decode {
        id: AssetId,
        path: Option<String>,
        reason: String,
    },
    #[error(
        "{}: a grid of {}x{} frames with margin {} and spacing {} does not cut the {}x{} image into whole frames",
        named(*id, path),
        grid.frame_width,
        grid.frame_height,
        grid.margin,
        grid.spacing,
        image.0,
        image.1
    )]
    Grid {
        id: AssetId,
        path: Option<String>,
        image: (u32, u32),
        grid: Grid,
    },
    #[error(
        "{}: a {}x{} frame with its 1-pixel extrusion does not fit a {max}x{max} atlas page",
        named(*id, path),
        size.0,
        size.1
    )]
    TooLarge {
        id: AssetId,
        path: Option<String>,
        size: (u32, u32),
        max: u32,
    },
    #[error("{}: invalid texture meta: {reason}", named(*id, path))]
    Meta {
        id: AssetId,
        path: Option<String>,
        reason: String,
    },
    /// Another source has the same identifier, so a sprite naming it could mean either: none
    /// of them is packed.
    #[error(
        "{}: another texture has the same identifier, so none of them is packed; give each its own meta",
        named(*id, path)
    )]
    Duplicate { id: AssetId, path: Option<String> },
}

impl TextureError {
    /// The identifier of the refused texture.
    pub fn id(&self) -> AssetId {
        self.origin().0
    }

    /// The project path of the refused texture, when known.
    pub fn path(&self) -> Option<&str> {
        self.origin().1.as_deref()
    }

    fn origin(&self) -> (AssetId, &Option<String>) {
        let (Self::Decode { id, path, .. }
        | Self::Grid { id, path, .. }
        | Self::TooLarge { id, path, .. }
        | Self::Meta { id, path, .. }
        | Self::Duplicate { id, path }) = self;
        (*id, path)
    }
}

/// "texture `<id>` (`<path>`)", or without the path when it is not known.
fn named(id: AssetId, path: &Option<String>) -> String {
    match path {
        Some(path) => format!("texture {id} (`{path}`)"),
        None => format!("texture {id}"),
    }
}

/// A decoded image in RGBA, 8 bits per channel.
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// A decoded texture cut into frames: each frame's top-left corner in the image, row-major,
/// and the region every frame shares until it is placed.
struct Texture {
    image: Image,
    corners: Vec<(u32, u32)>,
    region: Region,
}

/// A frame to place: its texture, index, and rectangle in the decoded image.
struct Frame<'a> {
    id: AssetId,
    index: u32,
    image: &'a Image,
    left: u32,
    top: u32,
    region: Region,
}

/// Decodes every source, cuts it into frames (row-major), extrudes each frame by 1 pixel, and
/// packs the frames by shelves, tallest first then by `(id, frame)`, into pages of at most
/// `max_page` pixels a side, opening a new page when one is full.
///
/// A texture that cannot be decoded, cut, or fitted, and every texture sharing its identifier
/// with another source, is left out and its error is kept in `Atlas::errors`, sorted by
/// identifier; the others still pack. When every source is refused the atlas has no page.
pub fn pack(sources: &[TextureSource<'_>], max_page: u32) -> Atlas {
    let mut counts: BTreeMap<AssetId, usize> = BTreeMap::new();
    for source in sources {
        *counts.entry(source.id).or_default() += 1;
    }
    let mut errors = Vec::new();
    let mut textures = Vec::new();
    for source in sources {
        let prepared = if counts[&source.id] > 1 {
            Err(TextureError::Duplicate {
                id: source.id,
                path: source.path.map(str::to_string),
            })
        } else {
            prepare(source, max_page)
        };
        match prepared {
            Ok(texture) => textures.push((source.id, texture)),
            Err(error) => errors.push(error),
        }
    }
    // The message names the path and the reason, so the order holds whatever the input order.
    errors.sort_by_cached_key(|error| (error.id(), error.to_string()));

    let mut frames: Vec<Frame<'_>> = textures
        .iter()
        .flat_map(|(id, texture)| {
            texture
                .corners
                .iter()
                .zip(0..)
                .map(|(&(left, top), index)| Frame {
                    id: *id,
                    index,
                    image: &texture.image,
                    left,
                    top,
                    region: texture.region,
                })
        })
        .collect();
    frames.sort_by_key(|frame| {
        (
            std::cmp::Reverse(frame.region.height),
            frame.id,
            frame.index,
        )
    });

    let mut shelves = Shelves::new(max_page);
    for frame in &mut frames {
        let (page, x, y) = shelves.place(frame.region.width + 2, frame.region.height + 2);
        frame.region.page = page;
        frame.region.x = x + 1;
        frame.region.y = y + 1;
    }

    let mut pages: Vec<AtlasPage> = shelves
        .extents
        .iter()
        .map(|&(width, height)| AtlasPage {
            width,
            height,
            rgba: vec![0; (width as usize) * (height as usize) * 4],
        })
        .collect();
    let mut regions = BTreeMap::new();
    for frame in &frames {
        blit_extruded(&mut pages[frame.region.page as usize], frame);
        regions.insert((frame.id, frame.index), frame.region);
    }
    Atlas {
        pages,
        regions,
        errors,
    }
}

/// Validates, decodes, and cuts one source.
fn prepare(source: &TextureSource<'_>, max_page: u32) -> Result<Texture, TextureError> {
    let id = source.id;
    let path = || source.path.map(str::to_string);
    let meta = source.meta;
    meta.validate().map_err(|reason| TextureError::Meta {
        id,
        path: path(),
        reason,
    })?;
    let image = decode(source.png).map_err(|reason| TextureError::Decode {
        id,
        path: path(),
        reason,
    })?;

    let grid = meta.grid.unwrap_or(Grid {
        frame_width: image.width,
        frame_height: image.height,
        margin: 0,
        spacing: 0,
    });
    let grid_error = || TextureError::Grid {
        id,
        path: path(),
        image: (image.width, image.height),
        grid,
    };
    let columns = whole_frames(image.width, grid.frame_width, grid.margin, grid.spacing)
        .ok_or_else(grid_error)?;
    let rows = whole_frames(image.height, grid.frame_height, grid.margin, grid.spacing)
        .ok_or_else(grid_error)?;
    if grid.frame_width.saturating_add(2) > max_page
        || grid.frame_height.saturating_add(2) > max_page
    {
        return Err(TextureError::TooLarge {
            id,
            path: path(),
            size: (grid.frame_width, grid.frame_height),
            max: max_page,
        });
    }

    let region = Region {
        page: 0,
        x: 0,
        y: 0,
        width: grid.frame_width,
        height: grid.frame_height,
        pivot: meta
            .pivot
            .unwrap_or((grid.frame_width / 2, grid.frame_height / 2)),
        pixels_per_unit: meta.pixels_per_unit,
        filter: meta.filter,
    };
    // Each term is at most the offset of a whole frame inside the image, so no sum exceeds the
    // image side. `frame + spacing` alone could overflow, so it is never computed.
    let corners = (0..rows)
        .flat_map(|row| (0..columns).map(move |column| (column, row)))
        .map(|(column, row)| {
            let left = grid.margin + column * grid.frame_width + column * grid.spacing;
            let top = grid.margin + row * grid.frame_height + row * grid.spacing;
            (left, top)
        })
        .collect();
    Ok(Texture {
        image,
        corners,
        region,
    })
}

/// How many frames of `frame` pixels, `spacing` apart and `margin` in from both edges, cut
/// `side` exactly; `None` when there is not one whole frame or a partial frame is left over.
fn whole_frames(side: u32, frame: u32, margin: u32, spacing: u32) -> Option<u32> {
    let inner = u64::from(side).checked_sub(2 * u64::from(margin))?;
    let step = u64::from(frame) + u64::from(spacing);
    let padded = inner + u64::from(spacing);
    if inner < u64::from(frame) || padded % step != 0 {
        return None;
    }
    // At most `side` frames, so the count fits a u32.
    u32::try_from(padded / step).ok()
}

/// Decodes a PNG of any color type and bit depth into 8-bit RGBA.
fn decode(bytes: &[u8]) -> Result<Image, String> {
    let limits = png::Limits {
        bytes: DECODE_LIMIT_BYTES,
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(bytes), limits);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let (width, height) = reader.info().size();
    let size = reader
        .output_buffer_size()
        .filter(|&size| size <= DECODE_LIMIT_BYTES)
        .ok_or_else(|| {
            format!(
                "the declared {width}x{height} image needs more than the {DECODE_LIMIT_BYTES} bytes a texture may take"
            )
        })?;
    let mut buffer = vec![0; size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| error.to_string())?;
    buffer.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .flat_map(|gray| [gray[0], gray[0], gray[0], gray[1]])
            .collect(),
        png::ColorType::Grayscale => buffer
            .iter()
            .flat_map(|&gray| [gray, gray, gray, 255])
            .collect(),
        png::ColorType::Indexed => return Err("the palette was not expanded".to_string()),
    };
    Ok(Image {
        width: info.width,
        height: info.height,
        rgba,
    })
}

/// Shelf packing: rectangles fill a shelf left to right, a shelf as tall as its first (and so
/// tallest) rectangle; a new shelf opens below, and a new page when no shelf fits.
struct Shelves {
    max: u32,
    /// Per page, the width and height used so far.
    extents: Vec<(u32, u32)>,
    /// The current shelf's top, height, and next free column.
    shelf: (u32, u32, u32),
}

impl Shelves {
    fn new(max: u32) -> Self {
        Shelves {
            max,
            extents: Vec::new(),
            shelf: (0, 0, 0),
        }
    }

    /// Places a `width` by `height` rectangle (at most `max` a side), returning its page and
    /// top-left corner.
    fn place(&mut self, width: u32, height: u32) -> (u32, u32, u32) {
        let (mut top, mut shelf_height, mut column) = self.shelf;
        if self.extents.is_empty() {
            self.extents.push((0, 0));
            shelf_height = height;
        } else if exceeds(column, width, self.max) {
            top += shelf_height;
            shelf_height = height;
            column = 0;
        }
        if exceeds(top, height, self.max) {
            self.extents.push((0, 0));
            top = 0;
            shelf_height = height;
            column = 0;
        }
        let page = self.extents.len() - 1;
        let extent = &mut self.extents[page];
        extent.0 = extent.0.max(column + width);
        extent.1 = extent.1.max(top + height);
        self.shelf = (top, shelf_height, column + width);
        // A page index never exceeds the number of frames, which a Vec bounds well below u32.
        (page as u32, column, top)
    }
}

/// Whether `start + length` passes `max`, computed so that a page side near `u32::MAX` cannot
/// overflow; once it does not pass, `start + length` fits a u32.
fn exceeds(start: u32, length: u32, max: u32) -> bool {
    u64::from(start) + u64::from(length) > u64::from(max)
}

/// Copies the frame into its region and repeats its edge pixels into the 1-pixel border, so
/// a filter sampling past the edge reads the frame, not its neighbour.
fn blit_extruded(page: &mut AtlasPage, frame: &Frame<'_>) {
    let region = frame.region;
    let image = frame.image;
    for y in 0..region.height + 2 {
        let source_y = frame.top + y.saturating_sub(1).min(region.height - 1);
        for x in 0..region.width + 2 {
            let source_x = frame.left + x.saturating_sub(1).min(region.width - 1);
            let from = ((source_y as usize) * (image.width as usize) + source_x as usize) * 4;
            let to = (((region.y + y - 1) as usize) * (page.width as usize)
                + (region.x + x - 1) as usize)
                * 4;
            page.rgba[to..to + 4].copy_from_slice(&image.rgba[from..from + 4]);
        }
    }
}

/// The version of the atlas files `Atlas::to_files` writes.
pub const ATLAS_VERSION: u32 = 1;

/// A packed atlas as files: `atlas.json` and one PNG per page, `page-<n>.png` for page `n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlasFiles {
    pub manifest: String,
    pub pages: Vec<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    pages: Vec<ManifestPage>,
    regions: Vec<ManifestRegion>,
}

#[derive(Serialize, Deserialize)]
struct ManifestPage {
    file: String,
    width: u32,
    height: u32,
}

#[derive(Serialize, Deserialize)]
struct ManifestRegion {
    id: AssetId,
    frame: u32,
    region: Region,
}

/// Why atlas files could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AtlasFileError {
    #[error("the atlas is version {found}, but this build reads up to version {supported}")]
    Version { found: u32, supported: u32 },
    #[error("invalid atlas manifest: {0}")]
    Json(String),
    #[error("the manifest lists page {index}, but no PNG was given for it")]
    MissingPage { index: usize },
    #[error(
        "page {index} is {}x{} in the manifest but its PNG is {}x{}",
        expected.0, expected.1, found.0, found.1
    )]
    PageSize {
        index: usize,
        expected: (u32, u32),
        found: (u32, u32),
    },
    #[error("frame {frame} of texture {id} lies outside page {page}")]
    RegionOutside { id: AssetId, frame: u32, page: u32 },
    #[error("cannot decode the PNG of page {index}: {reason}")]
    Decode { index: usize, reason: String },
}

impl Atlas {
    /// The atlas as files. Textures that were not packed (`errors`) are not written.
    ///
    /// Reproducible byte for byte: the manifest is pretty-printed with fields in declaration
    /// order and regions in `(id, frame)` order, and the pages are PNGs written with
    /// `Compression::Fast` and a fixed `Sub` filter. `Fast` is the `fdeflate` encoder, a
    /// single implementation pinned by `Cargo.lock` with no cargo feature to change its output,
    /// while `Balanced` and `High` go through `flate2`, whose backend features cargo unifies
    /// across the workspace. The adaptive filter is avoided so the bytes do not depend on a
    /// heuristic. A test pins the exact bytes, so a `png` or `fdeflate` upgrade is noticed.
    ///
    /// # Panics
    ///
    /// When a page breaks the invariant `pack` and `from_files` keep: a non-zero size and
    /// `width * height * 4` bytes of pixels (the fields are public, so a hand-built atlas can).
    pub fn to_files(&self) -> AtlasFiles {
        let manifest = Manifest {
            version: ATLAS_VERSION,
            pages: self
                .pages
                .iter()
                .zip(0..)
                .map(|(page, index)| ManifestPage {
                    file: format!("page-{index}.png"),
                    width: page.width,
                    height: page.height,
                })
                .collect(),
            regions: self
                .regions
                .iter()
                .map(|(&(id, frame), &region)| ManifestRegion { id, frame, region })
                .collect(),
        };
        AtlasFiles {
            // Plain structs of numbers and strings: serializing them cannot fail.
            manifest: serde_json::to_string_pretty(&manifest)
                .expect("a manifest of numbers and strings serializes"),
            pages: self.pages.iter().map(encode_page).collect(),
        }
    }

    /// Reads atlas files: the manifest and the PNG of each page it lists, in order (each
    /// page's `file` must be `page-<index>.png`; extra PNGs are ignored). The version is read
    /// first, then every page and region is checked. The result has no `errors`.
    pub fn from_files(manifest: &str, pages: &[&[u8]]) -> Result<Atlas, AtlasFileError> {
        let value: serde_json::Value = serde_json::from_str(manifest)
            .map_err(|error| AtlasFileError::Json(error.to_string()))?;
        let found = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
            .filter(|&version| version >= 1)
            .ok_or_else(|| {
                AtlasFileError::Json("`version` must be a whole number from 1".to_string())
            })?;
        if found > ATLAS_VERSION {
            return Err(AtlasFileError::Version {
                found,
                supported: ATLAS_VERSION,
            });
        }
        let manifest: Manifest = serde_json::from_value(value)
            .map_err(|error| AtlasFileError::Json(error.to_string()))?;

        let mut decoded = Vec::with_capacity(manifest.pages.len());
        for (index, listed) in manifest.pages.iter().enumerate() {
            let expected = format!("page-{index}.png");
            if listed.file != expected {
                return Err(AtlasFileError::Json(format!(
                    "page {index} must be the file `{expected}`, but the manifest names `{}`",
                    listed.file
                )));
            }
            let png = pages
                .get(index)
                .ok_or(AtlasFileError::MissingPage { index })?;
            let image = decode(png).map_err(|reason| AtlasFileError::Decode { index, reason })?;
            if (image.width, image.height) != (listed.width, listed.height) {
                return Err(AtlasFileError::PageSize {
                    index,
                    expected: (listed.width, listed.height),
                    found: (image.width, image.height),
                });
            }
            decoded.push(AtlasPage {
                width: image.width,
                height: image.height,
                rgba: image.rgba,
            });
        }

        let mut regions = BTreeMap::new();
        for ManifestRegion { id, frame, region } in manifest.regions {
            let fits = decoded.get(region.page as usize).is_some_and(|page| {
                region.width > 0
                    && region.height > 0
                    && u64::from(region.x) + u64::from(region.width) <= u64::from(page.width)
                    && u64::from(region.y) + u64::from(region.height) <= u64::from(page.height)
            });
            if !fits {
                return Err(AtlasFileError::RegionOutside {
                    id,
                    frame,
                    page: region.page,
                });
            }
            if region.pixels_per_unit == 0 {
                return Err(AtlasFileError::Json(format!(
                    "frame {frame} of texture {id} has pixels_per_unit 0"
                )));
            }
            if regions.insert((id, frame), region).is_some() {
                return Err(AtlasFileError::Json(format!(
                    "frame {frame} of texture {id} is listed twice"
                )));
            }
        }
        Ok(Atlas {
            pages: decoded,
            regions,
            errors: Vec::new(),
        })
    }
}

/// A page as an 8-bit RGBA PNG with a fixed encoder and filter (see `Atlas::to_files`).
fn encode_page(page: &AtlasPage) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, page.width, page.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.set_filter(png::Filter::Sub);
    // Pages come from `pack` or `from_files`: a non-zero size and `width * height * 4` bytes,
    // which are the only reasons encoding to memory fails.
    let mut writer = encoder.write_header().expect("a page has a PNG header");
    writer
        .write_image_data(&page.rgba)
        .expect("a page's pixels match its size");
    writer.finish().expect("a PNG ends in memory");
    bytes
}
