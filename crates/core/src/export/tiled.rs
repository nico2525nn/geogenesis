//! Streaming writers for tiled large-grid generation.
//!
//! A 4096² world has 16.7 million cells: a full RGB buffer for the terrain map
//! alone is 50 MB, a normal map another 50 MB, and the 16-bit heightmap needs
//! 67 MB of `f32` input. Tiled generation therefore never materialises an
//! image: the driver hands this module one tile at a time, in plan order, and
//! the writers emit rows as they arrive.
//!
//! PNG is written with the same dependency-free encoder primitives as
//! [`crate::export`] (uncompressed DEFLATE stored blocks), but streamed:
//! [`ScanlinePng`] accepts one image row per call and keeps only a small
//! pending IDAT chunk in memory. A zlib stream may be split across multiple
//! IDAT chunks and the stored blocks may be emitted incrementally, so arbitrary
//! image heights need no whole-image buffer; the final empty stored block and
//! the Adler-32 trailer are written by [`ScanlinePng::finish`].
//!
//! Two outputs need global information:
//!
//! * the 16-bit heightmap is normalised to the world minimum/maximum, which is
//!   only known after every tile has been generated. The tiled path streams the
//!   raw `f32` elevations to `elevation.f32` while tiles arrive and converts
//!   that file to PNG row by row at the end (deleting it again if the caller
//!   did not ask for the raw raster).
//! * the coarse preview map is small (typically 512²), so it is rendered from
//!   the box-filtered accumulator in one go.
//!
//! Memory use is independent of the image size: the largest buffers are one
//! tile row plus the pending IDAT chunk.

use crate::error::{GeoError, GeoResult};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// PNG magic bytes.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Format limit for one DEFLATE stored block.
const STORED_BLOCK_MAX: usize = 65_535;

/// IDAT chunks are flushed once this many zlib bytes are pending.
const IDAT_FLUSH_BYTES: usize = 256 * 1024;

/// Which streamed outputs a tiled pass writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportKinds {
    /// Hypsometric tint + hillshade `terrain.png`.
    pub terrain_png: bool,
    /// 16-bit grayscale heightmap `height16.png` (normalised to the world range).
    pub height16_png: bool,
    /// Raw little-endian `f32` elevations `elevation.f32`.
    pub elevation_f32: bool,
    /// Tangent-space style normal map `normals.png`.
    pub normals_png: bool,
}

impl Default for ExportKinds {
    fn default() -> Self {
        Self {
            terrain_png: true,
            height16_png: true,
            elevation_f32: true,
            normals_png: false,
        }
    }
}

impl ExportKinds {
    /// Every streamed output, including the normal map.
    pub const ALL: Self = Self {
        terrain_png: true,
        height16_png: true,
        elevation_f32: true,
        normals_png: true,
    };

    /// True when at least one output is requested.
    #[must_use]
    pub const fn any(self) -> bool {
        self.terrain_png || self.height16_png || self.elevation_f32 || self.normals_png
    }
}

/// One tiled export request: an output directory plus the formats to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRequest {
    /// Directory that receives the files (created if missing).
    pub dir: PathBuf,
    /// Streamed formats to write.
    pub kinds: ExportKinds,
}

impl ExportRequest {
    /// Request the default streamed outputs in `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            kinds: ExportKinds::default(),
        }
    }

    /// Request specific streamed outputs in `dir`.
    #[must_use]
    pub fn with_kinds(dir: impl Into<PathBuf>, kinds: ExportKinds) -> Self {
        Self {
            dir: dir.into(),
            kinds,
        }
    }
}

/// Files written by one tiled export pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// File names, in the order they were written.
    pub files: Vec<String>,
    /// Total bytes written (including a transient raw raster, if any).
    pub bytes: u64,
}

/// Streaming PNG writer (8/16-bit grayscale or 8-bit RGB) using uncompressed
/// DEFLATE stored blocks.
///
/// Rows are supplied without the PNG filter byte; the writer prepends filter
/// type 0 ("none"). Rows must be supplied top to bottom exactly `height` times.
#[derive(Debug)]
pub struct ScanlinePng {
    path: PathBuf,
    file: BufWriter<File>,
    height: u32,
    bytes_per_row: usize,
    rows_written: u32,
    // Running Adler-32 of the filtered (raw) zlib payload.
    adler_a: u32,
    adler_b: u32,
    // zlib bytes waiting to be emitted as one IDAT chunk.
    pending: Vec<u8>,
    bytes_written: u64,
}

impl ScanlinePng {
    /// Creates `path` and writes the PNG signature and IHDR chunk.
    ///
    /// `bit_depth` is 8 or 16; `colour_type` is 0 (grayscale) or 2 (RGB).
    pub fn create(
        path: &Path,
        width: u32,
        height: u32,
        bit_depth: u8,
        colour_type: u8,
    ) -> GeoResult<Self> {
        if width == 0 || height == 0 {
            return Err(GeoError::InvalidGrid(format!(
                "PNG `{}` needs non-zero dimensions, got {width}x{height}",
                path.display()
            )));
        }
        let bytes_per_pixel = match (bit_depth, colour_type) {
            (8, 0) => 1,
            (8, 2) => 3,
            (16, 0) => 2,
            _ => {
                return Err(GeoError::InvalidConfig(format!(
                    "unsupported PNG format: bit depth {bit_depth}, colour type {colour_type}"
                )));
            }
        };
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| io_error(parent, &e))?;
            }
        }
        let file = File::create(path).map_err(|e| io_error(path, &e))?;
        let mut png = Self {
            path: path.to_path_buf(),
            file: BufWriter::with_capacity(1 << 20, file),
            height,
            bytes_per_row: width as usize * bytes_per_pixel,
            rows_written: 0,
            adler_a: 1,
            adler_b: 0,
            pending: Vec::with_capacity(IDAT_FLUSH_BYTES + STORED_BLOCK_MAX),
            bytes_written: 0,
        };
        png.write_raw(&PNG_SIGNATURE)?;
        // zlib stream header: deflate with a 32 KiB window, no preset dictionary.
        png.pending.extend_from_slice(&[0x78, 0x01]);
        let mut ihdr = [0u8; 13];
        ihdr[..4].copy_from_slice(&width.to_be_bytes());
        ihdr[4..8].copy_from_slice(&height.to_be_bytes());
        ihdr[8] = bit_depth;
        ihdr[9] = colour_type;
        ihdr[10] = 0; // compression: deflate
        ihdr[11] = 0; // filter: adaptive
        ihdr[12] = 0; // interlace: none
        png.write_chunk(b"IHDR", &ihdr)?;
        Ok(png)
    }

    /// Number of rows still expected.
    #[must_use]
    pub fn rows_remaining(&self) -> u32 {
        self.height - self.rows_written
    }

    /// Appends one image row, without the filter byte.
    pub fn write_row(&mut self, pixels: &[u8]) -> GeoResult<()> {
        if pixels.len() != self.bytes_per_row {
            return Err(GeoError::InvalidConfig(format!(
                "PNG `{}`: row has {} bytes, expected {}",
                self.path.display(),
                pixels.len(),
                self.bytes_per_row
            )));
        }
        if self.rows_written >= self.height {
            return Err(GeoError::InvalidConfig(format!(
                "PNG `{}`: too many rows (height {})",
                self.path.display(),
                self.height
            )));
        }
        let filtered_len = pixels.len() + 1;
        if filtered_len > STORED_BLOCK_MAX {
            return Err(GeoError::InvalidGrid(format!(
                "PNG `{}`: row of {filtered_len} bytes exceeds one DEFLATE stored block",
                self.path.display()
            )));
        }
        // One stored block per row, always non-final; `finish` writes the final
        // empty block.
        self.pending.push(0x00); // BFINAL = 0, BTYPE = 00 (stored)
        self.pending
            .extend_from_slice(&(filtered_len as u16).to_le_bytes());
        self.pending
            .extend_from_slice(&(!(filtered_len as u16)).to_le_bytes());
        self.pending.push(0x00); // filter type: none
        self.pending.extend_from_slice(pixels);
        self.update_adler(&[0]);
        self.update_adler(pixels);
        self.rows_written += 1;
        if self.pending.len() >= IDAT_FLUSH_BYTES {
            self.flush_idat()?;
        }
        Ok(())
    }

    /// Writes the final DEFLATE block, the Adler-32 trailer and IEND, then
    /// flushes. Returns the number of bytes written to the file.
    pub fn finish(mut self) -> GeoResult<u64> {
        if self.rows_written != self.height {
            return Err(GeoError::InvalidConfig(format!(
                "PNG `{}`: {} of {} rows written",
                self.path.display(),
                self.rows_written,
                self.height
            )));
        }
        // Empty final stored block: BFINAL = 1, LEN = 0, NLEN = 0xFFFF.
        self.pending
            .extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
        let adler = (self.adler_b << 16) | self.adler_a;
        self.pending.extend_from_slice(&adler.to_be_bytes());
        self.flush_idat()?;
        self.write_chunk(b"IEND", &[])?;
        self.file.flush().map_err(|e| io_error(&self.path, &e))?;
        Ok(self.bytes_written)
    }

    fn flush_idat(&mut self) -> GeoResult<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.pending);
        let result = self.write_chunk(b"IDAT", &chunk);
        self.pending = chunk;
        self.pending.clear();
        result
    }

    fn write_raw(&mut self, bytes: &[u8]) -> GeoResult<()> {
        self.file
            .write_all(bytes)
            .map_err(|e| io_error(&self.path, &e))?;
        self.bytes_written += bytes.len() as u64;
        Ok(())
    }

    fn write_chunk(&mut self, kind: &[u8; 4], data: &[u8]) -> GeoResult<()> {
        let mut crc = 0xFFFF_FFFFu32;
        crc = crc32_update(crc, kind);
        crc = crc32_update(crc, data);
        self.file
            .write_all(&(data.len() as u32).to_be_bytes())
            .and_then(|()| self.file.write_all(kind))
            .and_then(|()| self.file.write_all(data))
            .and_then(|()| self.file.write_all(&(!crc).to_be_bytes()))
            .map_err(|e| io_error(&self.path, &e))?;
        self.bytes_written += 4 + 4 + data.len() as u64 + 4;
        Ok(())
    }

    fn update_adler(&mut self, data: &[u8]) {
        let (mut a, mut b) = (self.adler_a, self.adler_b);
        for &byte in data {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        self.adler_a = a;
        self.adler_b = b;
    }
}

/// Streams a full-resolution world to raster files, one tile at a time.
///
/// Tiles must be supplied in row-major plan order (`tile_y` increasing, and
/// within a tile row `tile_x` left to right); [`Self::write_tile`] rejects
/// anything else so a parallel scheduler cannot silently shuffle the output.
/// Every tile carries its halo, which the writer uses for the one-cell
/// gradients of the hillshade and normal map, so seams are shaded with values
/// from the neighbouring tile and no edge artefacts appear.
#[derive(Debug)]
pub struct TiledExportWriter {
    dir: PathBuf,
    width: u32,
    height: u32,
    sea_level_m: f64,
    cell_m_x: f64,
    cell_m_y: f64,
    wraps_x: bool,
    wraps_y: bool,
    kinds: ExportKinds,
    terrain: Option<ScanlinePng>,
    normals: Option<ScanlinePng>,
    raw: Option<BufWriter<File>>,
    raw_path: PathBuf,
    expected_x: u32,
    expected_y: u32,
    // Current tile-row band, assembled as tiles arrive left to right and flushed
    // to the writers once the band is complete. One band is a full-width slice of
    // the image (a few MB at 4096²), never the whole image.
    band_rows: u32,
    terrain_band: Vec<u8>,
    normals_band: Vec<u8>,
    raw_band: Vec<u8>,
}

impl TiledExportWriter {
    /// Creates the writers for `dir` and writes the PNG headers.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        dir: &Path,
        width: u32,
        height: u32,
        sea_level_m: f64,
        cell_m_x: f64,
        cell_m_y: f64,
        wraps_x: bool,
        wraps_y: bool,
        kinds: ExportKinds,
    ) -> GeoResult<Self> {
        if !kinds.any() {
            return Err(GeoError::InvalidConfig(
                "tiled export requested with no formats enabled".to_string(),
            ));
        }
        std::fs::create_dir_all(dir).map_err(|e| io_error(dir, &e))?;
        let terrain = if kinds.terrain_png {
            Some(ScanlinePng::create(
                &dir.join("terrain.png"),
                width,
                height,
                8,
                2,
            )?)
        } else {
            None
        };
        let normals = if kinds.normals_png {
            Some(ScanlinePng::create(
                &dir.join("normals.png"),
                width,
                height,
                8,
                2,
            )?)
        } else {
            None
        };
        let raw_path = dir.join("elevation.f32");
        let raw = if kinds.height16_png || kinds.elevation_f32 {
            Some(BufWriter::with_capacity(
                1 << 20,
                File::create(&raw_path).map_err(|e| io_error(&raw_path, &e))?,
            ))
        } else {
            None
        };
        Ok(Self {
            dir: dir.to_path_buf(),
            width,
            height,
            sea_level_m,
            cell_m_x,
            cell_m_y,
            wraps_x,
            wraps_y,
            kinds,
            terrain,
            normals,
            raw,
            raw_path,
            expected_x: 0,
            expected_y: 0,
            band_rows: 0,
            terrain_band: Vec::new(),
            normals_band: Vec::new(),
            raw_band: Vec::new(),
        })
    }

    /// Number of output rows already streamed.
    #[must_use]
    pub fn rows_written(&self) -> u32 {
        self.expected_y
    }

    /// Streams one padded tile of elevations: core plus `halo` cells on every
    /// side, `stride` cells per buffer row.
    #[allow(clippy::too_many_arguments)]
    pub fn write_tile(
        &mut self,
        tile_x: u32,
        tile_y: u32,
        tile_width: u32,
        tile_height: u32,
        buffer: &[f64],
        stride: usize,
        halo: u32,
    ) -> GeoResult<()> {
        if tile_x != self.expected_x || tile_y != self.expected_y {
            return Err(GeoError::InvalidConfig(format!(
                "tiled export out of order: got tile at ({tile_x}, {tile_y}), expected ({}, {})",
                self.expected_x, self.expected_y
            )));
        }
        if halo == 0 {
            return Err(GeoError::InvalidConfig(
                "tiled export needs at least a one-cell halo for gradients".to_string(),
            ));
        }
        let halo = halo as usize;
        let rows = tile_height as usize + 2 * halo;
        let needed = stride
            .checked_mul(rows)
            .ok_or_else(|| GeoError::InvalidConfig("tile buffer size overflow".to_string()))?;
        if buffer.len() < needed || stride < tile_width as usize + 2 * halo {
            return Err(GeoError::InvalidConfig(format!(
                "tile buffer too small: {} values, stride {stride}",
                buffer.len()
            )));
        }
        self.begin_band(tile_height)?;
        let gradient = GradientGeometry {
            width: self.width,
            height: self.height,
            wraps_x: self.wraps_x,
            wraps_y: self.wraps_y,
            cell_m_x: self.cell_m_x,
            cell_m_y: self.cell_m_y,
        };
        let band_width = self.width as usize;
        for ty in 0..tile_height as usize {
            let by = ty + halo;
            let gy = tile_y as usize + ty;
            let band_row = ty * band_width + tile_x as usize;
            // ---- Terrain RGB row segment.
            if self.terrain.is_some() {
                let band = &mut self.terrain_band;
                for tx in 0..tile_width as usize {
                    let bx = tx + halo;
                    let gx = tile_x as usize + tx;
                    let z = buffer[by * stride + bx];
                    let (dzdx, dzdy) = surface_gradient(&gradient, buffer, stride, bx, by, gx, gy);
                    let (r, g, b) = terrain_colour(z, self.sea_level_m, dzdx, dzdy);
                    let at = (band_row + tx) * 3;
                    band[at] = r;
                    band[at + 1] = g;
                    band[at + 2] = b;
                }
            }
            // ---- Normal map row segment.
            if self.normals.is_some() {
                let band = &mut self.normals_band;
                for tx in 0..tile_width as usize {
                    let bx = tx + halo;
                    let gx = tile_x as usize + tx;
                    let (dzdx, dzdy) = surface_gradient(&gradient, buffer, stride, bx, by, gx, gy);
                    let inv = 1.0 / (dzdx * dzdx + dzdy * dzdy + 1.0).sqrt();
                    let nx = -dzdx * inv;
                    let ny = -dzdy * inv;
                    let nz = inv;
                    let at = (band_row + tx) * 3;
                    band[at] = ((nx * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
                    band[at + 1] = ((ny * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
                    band[at + 2] = ((nz * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
            // ---- Raw f32 row segment.
            if self.raw.is_some() {
                let band = &mut self.raw_band;
                for tx in 0..tile_width as usize {
                    let z = buffer[by * stride + tx + halo];
                    let at = (band_row + tx) * 4;
                    band[at..at + 4].copy_from_slice(&(z as f32).to_le_bytes());
                }
            }
        }
        self.expected_x += tile_width;
        if self.expected_x >= self.width {
            if self.expected_x > self.width {
                return Err(GeoError::InvalidGrid(format!(
                    "tiled export tile row overflows the image ({} > {})",
                    self.expected_x, self.width
                )));
            }
            self.flush_band()?;
            self.expected_x = 0;
            self.expected_y += tile_height;
        }
        if self.expected_y > self.height {
            return Err(GeoError::InvalidGrid(format!(
                "tiled export wrote past the bottom of the image ({} rows)",
                self.expected_y
            )));
        }
        Ok(())
    }

    /// Prepares the band buffers for a new tile row (or checks the current one
    /// has the expected height).
    fn begin_band(&mut self, tile_height: u32) -> GeoResult<()> {
        if tile_height == 0 {
            return Err(GeoError::InvalidGrid(
                "tiled export tile has zero height".to_string(),
            ));
        }
        if self.band_rows != 0 {
            if self.band_rows != tile_height {
                return Err(GeoError::InvalidGrid(format!(
                    "tiled export row band height changed: {} -> {tile_height}",
                    self.band_rows
                )));
            }
            return Ok(());
        }
        self.band_rows = tile_height;
        let cells = self.width as usize * tile_height as usize;
        if self.kinds.terrain_png {
            self.terrain_band.clear();
            self.terrain_band.resize(cells * 3, 0);
        }
        if self.kinds.normals_png {
            self.normals_band.clear();
            self.normals_band.resize(cells * 3, 0);
        }
        if self.raw.is_some() {
            self.raw_band.clear();
            self.raw_band.resize(cells * 4, 0);
        }
        Ok(())
    }

    /// Emits the completed tile-row band to the writers.
    fn flush_band(&mut self) -> GeoResult<()> {
        let rows = self.band_rows;
        if rows == 0 {
            return Ok(());
        }
        let band_width = self.width as usize;
        if let Some(png) = self.terrain.as_mut() {
            for row in 0..rows as usize {
                let start = row * band_width * 3;
                png.write_row(&self.terrain_band[start..start + band_width * 3])?;
            }
        }
        if let Some(png) = self.normals.as_mut() {
            for row in 0..rows as usize {
                let start = row * band_width * 3;
                png.write_row(&self.normals_band[start..start + band_width * 3])?;
            }
        }
        if let Some(raw) = self.raw.as_mut() {
            for row in 0..rows as usize {
                let start = row * band_width * 4;
                raw.write_all(&self.raw_band[start..start + band_width * 4])
                    .map_err(|e| io_error(&self.raw_path, &e))?;
            }
        }
        self.band_rows = 0;
        Ok(())
    }

    /// Closes every writer, converts the raw raster to the 16-bit heightmap and
    /// returns the file list.
    pub fn finish(mut self, min_m: f64, max_m: f64) -> GeoResult<ExportSummary> {
        if self.expected_x != 0 || self.expected_y != self.height || self.band_rows != 0 {
            return Err(GeoError::InvalidConfig(format!(
                "tiled export finished with {} of {} rows",
                self.expected_y, self.height
            )));
        }
        let mut summary = ExportSummary::default();
        if let Some(png) = self.terrain.take() {
            summary.bytes += png.finish()?;
            summary.files.push("terrain.png".to_string());
        }
        if let Some(png) = self.normals.take() {
            summary.bytes += png.finish()?;
            summary.files.push("normals.png".to_string());
        }
        let mut raw_bytes = 0u64;
        if let Some(mut raw) = self.raw.take() {
            raw.flush().map_err(|e| io_error(&self.raw_path, &e))?;
            drop(raw);
            raw_bytes = std::fs::metadata(&self.raw_path)
                .map(|m| m.len())
                .unwrap_or(0);
            summary.bytes += raw_bytes;
            if self.kinds.elevation_f32 {
                summary.files.push("elevation.f32".to_string());
            }
        }
        if self.kinds.height16_png {
            let path = self.dir.join("height16.png");
            summary.bytes +=
                height16_from_raw(&self.raw_path, &path, self.width, self.height, min_m, max_m)?;
            summary.files.push("height16.png".to_string());
            if !self.kinds.elevation_f32 {
                // The raw raster was only an intermediate for the heightmap.
                std::fs::remove_file(&self.raw_path).map_err(|e| io_error(&self.raw_path, &e))?;
                summary.bytes -= raw_bytes;
            }
        }
        Ok(summary)
    }
}

/// Grid geometry needed for edge-aware gradients, copied out of the writer so
/// the row loops can read it while a PNG writer is mutably borrowed.
#[derive(Clone, Copy, Debug)]
struct GradientGeometry {
    width: u32,
    height: u32,
    wraps_x: bool,
    wraps_y: bool,
    cell_m_x: f64,
    cell_m_y: f64,
}

/// Central-difference surface gradient, one-sided at open world edges.
#[allow(clippy::too_many_arguments)]
fn surface_gradient(
    geometry: &GradientGeometry,
    buffer: &[f64],
    stride: usize,
    bx: usize,
    by: usize,
    gx: usize,
    gy: usize,
) -> (f64, f64) {
    let centre = by * stride + bx;
    let z = buffer[centre];
    let dzdx = if !geometry.wraps_x && gx == 0 {
        (buffer[centre + 1] - z) / geometry.cell_m_x
    } else if !geometry.wraps_x && gx + 1 >= geometry.width as usize {
        (z - buffer[centre - 1]) / geometry.cell_m_x
    } else {
        (buffer[centre + 1] - buffer[centre - 1]) / (2.0 * geometry.cell_m_x)
    };
    let dzdy = if !geometry.wraps_y && gy == 0 {
        (buffer[centre + stride] - z) / geometry.cell_m_y
    } else if !geometry.wraps_y && gy + 1 >= geometry.height as usize {
        (z - buffer[centre - stride]) / geometry.cell_m_y
    } else {
        (buffer[centre + stride] - buffer[centre - stride]) / (2.0 * geometry.cell_m_y)
    };
    (dzdx, dzdy)
}

/// Height (or depth) colour with a north-west hillshade, matching
/// [`crate::export::colour_map`]'s terrain mode without rivers or lakes.
fn terrain_colour(z: f64, sea: f64, dzdx: f64, dzdy: f64) -> (u8, u8, u8) {
    if z <= sea {
        // Ocean: deep navy abyss → teal shelf.
        let depth = ((sea - z) / 6000.0).clamp(0.0, 1.0);
        let shallow = 1.0 - depth;
        let r = (8.0 + 22.0 * shallow) as u8;
        let g = (28.0 + 68.0 * shallow) as u8;
        let b = (66.0 + 86.0 * shallow) as u8;
        return (r, g, b);
    }
    let above = z - sea;
    let t = (above / 2600.0).clamp(0.0, 1.0);
    let (mut r, mut g, mut b) = if t < 0.04 {
        (
            198.0 - 30.0 * t / 0.04,
            186.0 - 40.0 * t / 0.04,
            146.0 - 50.0 * t / 0.04,
        )
    } else if t < 0.30 {
        let k = (t - 0.04) / 0.26;
        (150.0 - 46.0 * k, 158.0 + 6.0 * k, 96.0 - 16.0 * k)
    } else if t < 0.62 {
        let k = (t - 0.30) / 0.32;
        (104.0 + 6.0 * k, 164.0 - 28.0 * k, 80.0 + 8.0 * k)
    } else if t < 0.85 {
        let k = (t - 0.62) / 0.23;
        (110.0 + 54.0 * k, 136.0 - 30.0 * k, 88.0 + 26.0 * k)
    } else {
        let k = (t - 0.85) / 0.15;
        (164.0 + 80.0 * k, 106.0 + 130.0 * k, 114.0 + 126.0 * k)
    };
    let shade = (-dzdx * 0.7071 - dzdy * 0.7071).clamp(-1.0, 1.0);
    let light = 0.78 + 0.40 * shade;
    r *= light;
    g *= light;
    b *= light;
    (
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    )
}

/// Converts the streamed raw `f32` raster into a 16-bit grayscale PNG,
/// normalised to `[min_m, max_m]` over the whole world.
fn height16_from_raw(
    raw_path: &Path,
    out_path: &Path,
    width: u32,
    height: u32,
    min_m: f64,
    max_m: f64,
) -> GeoResult<u64> {
    let file = File::open(raw_path).map_err(|e| io_error(raw_path, &e))?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut png = ScanlinePng::create(out_path, width, height, 16, 0)?;
    let span = (max_m - min_m).max(1e-9);
    let mut bytes = vec![0u8; width as usize * 4];
    let mut pixels = vec![0u8; width as usize * 2];
    for _ in 0..height {
        reader
            .read_exact(&mut bytes)
            .map_err(|e| io_error(raw_path, &e))?;
        for x in 0..width as usize {
            let z = f64::from(f32::from_le_bytes([
                bytes[x * 4],
                bytes[x * 4 + 1],
                bytes[x * 4 + 2],
                bytes[x * 4 + 3],
            ]));
            let value = (((z - min_m) / span) * 65_535.0)
                .round()
                .clamp(0.0, 65_535.0) as u16;
            pixels[x * 2..x * 2 + 2].copy_from_slice(&value.to_be_bytes());
        }
        png.write_row(&pixels)?;
    }
    png.finish()
}

/// Renders a box-filtered preview (elevation sums plus per-cell counts, as
/// accumulated by the tiled driver) as a hypsometric RGB map.
#[must_use]
pub fn preview_rgb(
    preview: &[f64],
    counts: &[u32],
    width: u32,
    height: u32,
    sea_level_m: f64,
) -> Vec<u8> {
    let mut pixels = vec![0u8; width as usize * height as usize * 3];
    for index in 0..(width as usize * height as usize).min(preview.len()) {
        let coverage = counts.get(index).copied().unwrap_or(0).max(1) as f64;
        let mean = preview[index] / coverage;
        let (r, g, b) = if mean <= sea_level_m {
            let depth = ((sea_level_m - mean) / 4000.0).clamp(0.0, 1.0);
            (
                (30.0 + 20.0 * (1.0 - depth)) as u8,
                (70.0 + 40.0 * (1.0 - depth)) as u8,
                (120.0 + 50.0 * (1.0 - depth)) as u8,
            )
        } else {
            let land = ((mean - sea_level_m) / 3000.0).clamp(0.0, 1.0);
            (
                (70.0 + 170.0 * land) as u8,
                (120.0 + 120.0 * land) as u8,
                (60.0 + 170.0 * land) as u8,
            )
        };
        pixels[index * 3] = r;
        pixels[index * 3 + 1] = g;
        pixels[index * 3 + 2] = b;
    }
    pixels
}

/// Writes the coarse preview as `preview.png` in `dir`; returns the file size.
pub fn write_preview_png(
    dir: &Path,
    preview: &[f64],
    counts: &[u32],
    width: u32,
    height: u32,
    sea_level_m: f64,
) -> GeoResult<u64> {
    let rgb = preview_rgb(preview, counts, width, height, sea_level_m);
    let png = crate::export::encode_png_rgb(width, height, &rgb);
    let path = dir.join("preview.png");
    let bytes = png.len() as u64;
    crate::export::write_bytes(&path, &png)?;
    Ok(bytes)
}

fn io_error(path: &Path, error: &std::io::Error) -> GeoError {
    GeoError::Io(format!("{}: {error}", path.display()))
}

/// Incremental CRC-32 (reflected, polynomial 0xEDB8_8320); start with
/// `0xFFFF_FFFF` and complement the result.
fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "geogenesis_tiled_export_{}_{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Minimal PNG reader for stored-block images written by this module.
    struct DecodedPng {
        width: u32,
        height: u32,
        bit_depth: u8,
        colour_type: u8,
        raw: Vec<u8>,
        idat_chunks: usize,
    }

    fn decode_stored_png(bytes: &[u8]) -> DecodedPng {
        assert_eq!(&bytes[..8], &PNG_SIGNATURE, "signature");
        let mut offset = 8usize;
        let mut width = 0;
        let mut height = 0;
        let mut bit_depth = 0;
        let mut colour_type = 0;
        let mut idat = Vec::new();
        let mut idat_chunks = 0;
        let mut saw_end = false;
        while offset < bytes.len() {
            let len = u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]) as usize;
            let kind = &bytes[offset + 4..offset + 8];
            let data = &bytes[offset + 8..offset + 8 + len];
            let crc_at = offset + 8 + len;
            let stored_crc = u32::from_be_bytes([
                bytes[crc_at],
                bytes[crc_at + 1],
                bytes[crc_at + 2],
                bytes[crc_at + 3],
            ]);
            let expected = !crc32_update(crc32_update(0xFFFF_FFFF, kind), data);
            assert_eq!(stored_crc, expected, "chunk CRC");
            match kind {
                b"IHDR" => {
                    width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                    height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                    bit_depth = data[8];
                    colour_type = data[9];
                }
                b"IDAT" => {
                    idat.extend_from_slice(data);
                    idat_chunks += 1;
                }
                b"IEND" => saw_end = true,
                _ => {}
            }
            offset = crc_at + 4;
        }
        assert!(saw_end, "missing IEND");
        // Inflate the zlib stream (stored blocks only).
        assert_eq!(&idat[..2], &[0x78, 0x01], "zlib header");
        let mut raw = Vec::new();
        let mut at = 2usize;
        loop {
            let header = idat[at];
            at += 1;
            let final_block = header & 1 != 0;
            assert_eq!((header >> 1) & 3, 0, "expected stored blocks");
            let len = u16::from_le_bytes([idat[at], idat[at + 1]]) as usize;
            let nlen = u16::from_le_bytes([idat[at + 2], idat[at + 3]]);
            assert_eq!(nlen, !(len as u16));
            at += 4;
            raw.extend_from_slice(&idat[at..at + len]);
            at += len;
            if final_block {
                break;
            }
        }
        let stored_adler = u32::from_be_bytes([idat[at], idat[at + 1], idat[at + 2], idat[at + 3]]);
        at += 4;
        assert_eq!(at, idat.len(), "trailing zlib bytes");
        let mut a = 1u32;
        let mut b = 0u32;
        for &byte in &raw {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        assert_eq!(stored_adler, (b << 16) | a, "Adler-32 trailer");
        DecodedPng {
            width,
            height,
            bit_depth,
            colour_type,
            raw,
            idat_chunks,
        }
    }

    #[test]
    fn scanline_png_streams_known_pixels() {
        let dir = temp_dir("scanline");
        let path = dir.join("test.png");
        let (w, h) = (7u32, 5u32);
        let mut png = ScanlinePng::create(&path, w, h, 8, 2).unwrap();
        for y in 0..h {
            let mut row = Vec::with_capacity(w as usize * 3);
            for x in 0..w {
                row.push((x + y * w) as u8);
                row.push((x * 2 + y) as u8);
                row.push(255);
            }
            png.write_row(&row).unwrap();
        }
        assert_eq!(png.rows_remaining(), 0);
        let written = png.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len() as u64, written);
        let decoded = decode_stored_png(&bytes);
        assert_eq!((decoded.width, decoded.height), (w, h));
        assert_eq!(decoded.bit_depth, 8);
        assert_eq!(decoded.colour_type, 2);
        assert_eq!(decoded.raw.len(), (w as usize * 3 + 1) * h as usize);
        assert!(decoded
            .raw
            .iter()
            .step_by(w as usize * 3 + 1)
            .all(|&f| f == 0));
        let row0 = &decoded.raw[1..1 + w as usize * 3];
        assert_eq!(row0[..6], [0, 0, 255, 1, 2, 255]);
        let last_start = (h as usize - 1) * (w as usize * 3 + 1) + 1;
        let last = &decoded.raw[last_start..last_start + w as usize * 3];
        assert_eq!(last[0], (w * (h - 1)) as u8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanline_png_splits_large_images_across_idat_chunks() {
        let dir = temp_dir("multi_idat");
        let path = dir.join("big.png");
        let (w, h) = (300u32, 600u32);
        let mut png = ScanlinePng::create(&path, w, h, 8, 2).unwrap();
        for y in 0..h {
            let mut row = vec![0u8; w as usize * 3];
            for x in 0..w as usize {
                row[x * 3] = (x % 251) as u8;
                row[x * 3 + 1] = (y % 251) as u8;
                row[x * 3 + 2] = ((x + y as usize) % 251) as u8;
            }
            png.write_row(&row).unwrap();
        }
        png.finish().unwrap();
        let decoded = decode_stored_png(&std::fs::read(&path).unwrap());
        assert!(decoded.idat_chunks >= 2, "expected several IDAT chunks");
        assert_eq!(decoded.raw.len(), (300 * 3 + 1) * 600);
        let row_at = |y: usize| -> &[u8] {
            let start = y * (300 * 3 + 1) + 1;
            &decoded.raw[start..start + 300 * 3]
        };
        assert_eq!(row_at(0)[0], 0);
        assert_eq!(row_at(599)[1], (599 % 251) as u8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scanline_png_rejects_bad_rows() {
        let dir = temp_dir("bad_rows");
        let path = dir.join("bad.png");
        let mut png = ScanlinePng::create(&path, 4, 2, 8, 2).unwrap();
        assert!(png.write_row(&[0u8; 5]).is_err());
        png.write_row(&[0u8; 12]).unwrap();
        png.write_row(&[0u8; 12]).unwrap();
        assert!(png.write_row(&[0u8; 12]).is_err());
        // Finishing early is an error, not a corrupt file.
        let early = ScanlinePng::create(&path, 4, 2, 8, 2).unwrap();
        assert!(early.finish().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Padded tile buffer with a deterministic, position-dependent pattern:
    /// value = seed + 10 * x + 100 * y in core coordinates.
    fn padded_tile(width: u32, height: u32, halo: u32, seed: f64) -> Vec<f64> {
        let stride = (width + 2 * halo) as usize;
        let rows = (height + 2 * halo) as usize;
        let mut buffer = vec![0.0; stride * rows];
        for y in 0..rows {
            for x in 0..stride {
                let gx = x as f64 - halo as f64;
                let gy = y as f64 - halo as f64;
                buffer[y * stride + x] = seed + 10.0 * gx + 100.0 * gy;
            }
        }
        buffer
    }

    #[test]
    fn exporter_streams_tiles_raw_height_and_normals() {
        let dir = temp_dir("exporter");
        let (w, h) = (8u32, 6u32);
        let halo = 2u32;
        let stride = (w + 2 * halo) as usize;
        let kinds = ExportKinds {
            terrain_png: true,
            height16_png: true,
            elevation_f32: true,
            normals_png: true,
        };
        let mut writer =
            TiledExportWriter::create(&dir, w, h, 0.0, 1000.0, 1000.0, true, true, kinds).unwrap();
        let top = padded_tile(w, 3, halo, 0.0);
        writer.write_tile(0, 0, w, 3, &top, stride, halo).unwrap();
        let bottom = padded_tile(w, 3, halo, 1.0);
        writer
            .write_tile(0, 3, w, 3, &bottom, stride, halo)
            .unwrap();
        assert_eq!(writer.rows_written(), h);
        let summary = writer.finish(0.0, 271.0).unwrap();
        assert_eq!(
            summary.files,
            vec![
                "terrain.png".to_string(),
                "normals.png".to_string(),
                "elevation.f32".to_string(),
                "height16.png".to_string()
            ]
        );
        assert!(summary.bytes > 0);
        let terrain = decode_stored_png(&std::fs::read(dir.join("terrain.png")).unwrap());
        assert_eq!((terrain.width, terrain.height), (w, h));
        let height = decode_stored_png(&std::fs::read(dir.join("height16.png")).unwrap());
        assert_eq!((height.width, height.height), (w, h));
        assert_eq!(height.bit_depth, 16);
        assert_eq!(height.colour_type, 0);
        assert_eq!(height.raw.len(), (8 * 2 + 1) * 6);
        // The raw raster round-trips the core elevations (as f32).
        let raw = std::fs::read(dir.join("elevation.f32")).unwrap();
        assert_eq!(raw.len(), 8 * 6 * 4);
        let first = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        assert_eq!(
            f64::from(first),
            top[halo as usize * stride + halo as usize]
        );
        let last_at = (6 - 1) * 8 * 4 + 7 * 4;
        let last = f32::from_le_bytes([
            raw[last_at],
            raw[last_at + 1],
            raw[last_at + 2],
            raw[last_at + 3],
        ]);
        assert_eq!(f64::from(last), 1.0 + 70.0 + 200.0);
        // Heightmap normalisation: the world minimum maps to 0, the maximum to
        // 65535 (both lie in the core region we selected).
        let first_height = u16::from_be_bytes([height.raw[1], height.raw[2]]);
        assert_eq!(first_height, 0);
        let last_height_at = (6 - 1) * (16 + 1) + 1 + 7 * 2;
        let last_height =
            u16::from_be_bytes([height.raw[last_height_at], height.raw[last_height_at + 1]]);
        assert_eq!(last_height, 65_535);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exporter_rejects_out_of_order_tiles() {
        let dir = temp_dir("order");
        let halo = 2u32;
        let stride = 4 + 2 * halo as usize;
        let mut writer = TiledExportWriter::create(
            &dir,
            8,
            4,
            0.0,
            1000.0,
            1000.0,
            true,
            true,
            ExportKinds {
                terrain_png: true,
                height16_png: false,
                elevation_f32: false,
                normals_png: false,
            },
        )
        .unwrap();
        let buffer = padded_tile(4, 4, halo, 0.0);
        assert!(writer
            .write_tile(4, 0, 4, 4, &buffer, stride, halo)
            .is_err());
        writer
            .write_tile(0, 0, 4, 4, &buffer, stride, halo)
            .unwrap();
        assert!(writer
            .write_tile(0, 0, 4, 4, &buffer, stride, halo)
            .is_err());
        writer
            .write_tile(4, 0, 4, 4, &buffer, stride, halo)
            .unwrap();
        assert!(writer.finish(0.0, 1.0).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transient_raw_raster_is_removed_when_not_requested() {
        let dir = temp_dir("transient_raw");
        let halo = 2u32;
        let stride = 8 + 2 * halo as usize;
        let kinds = ExportKinds {
            terrain_png: false,
            height16_png: true,
            elevation_f32: false,
            normals_png: false,
        };
        let mut writer =
            TiledExportWriter::create(&dir, 8, 4, 0.0, 1000.0, 1000.0, true, true, kinds).unwrap();
        let buffer = padded_tile(8, 4, halo, 0.0);
        writer
            .write_tile(0, 0, 8, 4, &buffer, stride, halo)
            .unwrap();
        let summary = writer.finish(0.0, 400.0).unwrap();
        assert_eq!(summary.files, vec!["height16.png".to_string()]);
        assert!(!dir.join("elevation.f32").exists());
        assert!(dir.join("height16.png").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_png_is_a_small_hypsometric_map() {
        let dir = temp_dir("preview");
        let preview = vec![100.0, -200.0, 0.0, 3000.0];
        let counts = vec![4u32, 4, 4, 4];
        let bytes = write_preview_png(&dir, &preview, &counts, 2, 2, 0.0).unwrap();
        assert!(bytes > 0);
        let decoded = decode_stored_png(&std::fs::read(dir.join("preview.png")).unwrap());
        assert_eq!((decoded.width, decoded.height), (2, 2));
        // Row 1: pixel 0 is sea level (ocean blue), pixel 1 is 750 m land.
        let row1 = &decoded.raw[1 + 2 * 3 + 1..];
        assert!(row1[3] > row1[0], "land should be redder than ocean");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crc32_matches_the_known_check_value() {
        assert_eq!(!crc32_update(0xFFFF_FFFF, b"123456789"), 0xCBF4_3926);
    }
}
