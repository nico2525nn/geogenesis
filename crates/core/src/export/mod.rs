//! Export formats (spec §12 Phase 3): raw fields, PNG maps, OBJ meshes, XYZ
//! point clouds, OpenEXR height textures and NetCDF-3 classic field files.
//!
//! The encoders are dependency-free by design: the engine ships as a small
//! WASM module, and the CLI/desktop exports should not drag a compression or
//! image-encoding crate into the core. PNG uses uncompressed DEFLATE stored
//! blocks, which every decoder accepts; the files are larger than with real
//! compression, but deterministic and trivially correct.
//!
//! EXR and NetCDF are similarly self-contained: [`exr::exr_heightmap`] writes
//! uncompressed single-part scanline images (float or half), and
//! [`netcdf::write_netcdf`] writes CDF-1 classic files with big-endian data, so
//! both formats open in standard HDR imaging, GIS and netCDF tools without
//! linking any codec or compression library.
//!
//! The live Web UI renders with WebGL and exports screenshots; these formats
//! are for files (CLI, batch runs, GIS).

pub mod exr;
pub mod netcdf;
pub mod tiled;

use crate::climate::Climate;
use crate::error::{GeoError, GeoResult};
use crate::hydrology::Hydro;
use crate::params::Params;
use crate::world::World;
use std::fs::File;
use std::io::Write;
use std::path::Path;

/// Colour mapping mode for raster exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapColour {
    /// Hypsometric tint with hillshade.
    Terrain,
    /// Elevation as grayscale.
    Elevation,
    /// Biome colours.
    Biomes,
    /// Slope as grayscale.
    Slope,
    /// Exposed rock colours.
    Rock,
    /// Rivers and lakes.
    Rivers,
}

/// Encodes a raw little-endian `f32` elevation raster.
#[must_use]
pub fn elevation_raw_f32(world: &World) -> Vec<u8> {
    let mut out = Vec::with_capacity(world.cell_count() * 4);
    for &z in world.elevation_m.as_slice() {
        out.extend_from_slice(&(z as f32).to_le_bytes());
    }
    out
}

/// Encodes a raw little-endian `f32` raster from any field slice.
#[must_use]
pub fn field_raw_f32(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for &v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Renders a coloured map into an RGB buffer.
#[must_use]
pub fn colour_map(
    world: &World,
    climate: Option<&Climate>,
    geology: Option<&crate::geology::Geology>,
    hydro: Option<&Hydro>,
    colour: MapColour,
) -> Vec<u8> {
    let grid = world.grid;
    let sea = world.sea_level_m;
    let (min, max) = world.relief_range_m();
    let span = (max - min).max(1.0);
    let mut rgb = vec![0u8; grid.cell_count() * 3];
    for y in 0..grid.height() {
        for x in 0..grid.width() {
            let i = grid.index(x, y);
            let z = world.elevation_m.get(x, y);
            let (r, g, b) = match colour {
                MapColour::Elevation => {
                    let v = (((z - min) / span) * 255.0).clamp(0.0, 255.0) as u8;
                    (v, v, v)
                }
                MapColour::Slope => {
                    let s = world.elevation_m.slope(x, y).atan().to_degrees();
                    let v = ((s / 60.0) * 255.0).clamp(0.0, 255.0) as u8;
                    (v, v, v)
                }
                MapColour::Biomes => match climate {
                    Some(c) => {
                        let biome = crate::climate::Biome::from_u8(c.biome.get(x, y));
                        let [r, g, b] = biome.colour();
                        (r, g, b)
                    }
                    None => (80, 80, 80),
                },
                MapColour::Rock => match geology {
                    Some(g) => {
                        let rock = g.rock_at(x, y);
                        (rock.colour[0], rock.colour[1], rock.colour[2])
                    }
                    None => (90, 90, 90),
                },
                MapColour::Rivers => match hydro {
                    Some(h) => {
                        if h.river_mask.get(x, y) > 0 {
                            (70, 130, 190)
                        } else if h.lake_depth_m.get(x, y) > 0.5 {
                            (50, 100, 150)
                        } else if z <= sea {
                            (24, 40, 64)
                        } else {
                            (60, 64, 60)
                        }
                    }
                    None => (60, 60, 60),
                },
                MapColour::Terrain => terrain_pixel(world, hydro, x, y, z, sea, min, span),
            };
            rgb[i * 3] = r;
            rgb[i * 3 + 1] = g;
            rgb[i * 3 + 2] = b;
        }
    }
    let _ = grid.cell_count();
    rgb
}

/// Hypsometric tint plus hillshade, water and rivers.
fn terrain_pixel(
    world: &World,
    hydro: Option<&Hydro>,
    x: usize,
    y: usize,
    z: f64,
    sea: f64,
    _min: f64,
    _span: f64,
) -> (u8, u8, u8) {
    // Rivers are drawn last so they read as line work on top of the tint.
    if let Some(h) = hydro {
        if h.river_mask.get(x, y) > 0 {
            let depth = z.max(sea);
            let _ = depth;
            return (64, 118, 158);
        }
        if h.lake_depth_m.get(x, y) > 1.0 {
            return (52, 102, 140);
        }
    }
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
        // Beach and low plains.
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
    // Hillshade from the north-west.
    let (dzdx, dzdy) = world.elevation_m.gradient(x, y);
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

/// Encodes an 8-bit grayscale PNG.
#[must_use]
pub fn encode_png_gray(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(
        pixels.len(),
        width as usize * height as usize,
        "pixel count"
    );
    encode_png(width, height, 0, pixels, width as usize)
}

/// Encodes an 8-bit RGB PNG.
#[must_use]
pub fn encode_png_rgb(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(
        pixels.len(),
        width as usize * height as usize * 3,
        "pixel count"
    );
    encode_png(width, height, 2, pixels, width as usize * 3)
}

/// Encodes a 16-bit grayscale PNG (big-endian samples).
#[must_use]
pub fn encode_png_gray16(width: u32, height: u32, pixels: &[u16]) -> Vec<u8> {
    assert_eq!(
        pixels.len(),
        width as usize * height as usize,
        "pixel count"
    );
    let mut raw = Vec::with_capacity((width as usize * 2 + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0); // filter: none
        for column in 0..width as usize {
            raw.extend_from_slice(&pixels[row * width as usize + column].to_be_bytes());
        }
    }
    encode_png_with_depth(width, height, 16, 0, &raw, width as usize * 2)
}

/// 16-bit heightmap of the world (min/max normalised to the full 0..65535
/// range), suitable for GIS and terrain import tools.
#[must_use]
pub fn height_png16(world: &World) -> Vec<u8> {
    let grid = world.grid;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &z in world.elevation_m.as_slice() {
        min = min.min(z);
        max = max.max(z);
    }
    let span = (max - min).max(1e-9);
    let pixels: Vec<u16> = world
        .elevation_m
        .as_slice()
        .iter()
        .map(|&z| (((z - min) / span) * 65_535.0).round().clamp(0.0, 65_535.0) as u16)
        .collect();
    encode_png_gray16(grid.width() as u32, grid.height() as u32, &pixels)
}

/// Tangent-space style normal map from the terrain gradients, rendered as an
/// 8-bit RGB PNG (flat = 128,128,255).
#[must_use]
pub fn normal_map_png(world: &World) -> Vec<u8> {
    let grid = world.grid;
    let w = grid.width();
    let h = grid.height();
    let cell = grid.cell_size_m().max(1.0);
    let mut rgb = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let left = world.elevation_m.get(grid.clamp_x(x as isize - 1), y);
            let right = world.elevation_m.get(grid.clamp_x(x as isize + 1), y);
            let down = world.elevation_m.get(x, grid.clamp_y(y as isize - 1));
            let up = world.elevation_m.get(x, grid.clamp_y(y as isize + 1));
            let gx = (right - left) / (2.0 * cell);
            let gy = (up - down) / (2.0 * cell);
            // Normalise (-gx, -gy, 1) so the colour encodes the true slope.
            let inv = 1.0 / (gx * gx + gy * gy + 1.0).sqrt();
            let nx = -gx * inv;
            let ny = -gy * inv;
            let nz = inv;
            let at = (y * w + x) * 3;
            rgb[at] = ((nx * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgb[at + 1] = ((ny * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgb[at + 2] = ((nz * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    encode_png_rgb(w as u32, h as u32, &rgb)
}

fn encode_png(width: u32, height: u32, colour_type: u8, pixels: &[u8], stride: usize) -> Vec<u8> {
    encode_png_with_depth(width, height, 8, colour_type, pixels, stride)
}

/// Encodes a PNG with explicit bit depth (8 or 16) and raw filtered rows.
fn encode_png_with_depth(
    width: u32,
    height: u32,
    bit_depth: u8,
    colour_type: u8,
    pixels: &[u8],
    stride: usize,
) -> Vec<u8> {
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0); // filter: none
        raw.extend_from_slice(&pixels[row * stride..(row + 1) * stride]);
    }
    let zlib = deflate_stored(&raw);

    let mut out = Vec::with_capacity(zlib.len() + 128);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(bit_depth);
    ihdr.push(colour_type);
    ihdr.extend_from_slice(&[0, 0, 0]); // compression, filter, interlace
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &zlib);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// zlib stream with uncompressed DEFLATE stored blocks.
fn deflate_stored(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 65_535 * 5 + 16);
    out.extend_from_slice(&[0x78, 0x01]); // zlib header: deflate, no dict
    let mut offset = 0usize;
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    }
    while offset < data.len() {
        let len = (data.len() - offset).min(65_535);
        let final_block = offset + len >= data.len();
        out.push(if final_block { 0x01 } else { 0x00 });
        let nlen = !(len as u16);
        out.extend_from_slice(&(len as u16).to_le_bytes());
        out.extend_from_slice(&nlen.to_le_bytes());
        out.extend_from_slice(&data[offset..offset + len]);
        offset += len;
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

/// Renders a cross-section profile as an RGB PNG.
///
/// The image shows the surface line, hypsometric terrain fill, water fill below
/// sea level, a dashed sea-level reference and the horizontal distance axis.
#[must_use]
pub fn profile_png(world: &World, samples: &[crate::stats::ProfileSample]) -> Vec<u8> {
    const WIDTH: usize = 1024;
    const HEIGHT: usize = 320;
    let sea = world.sea_level_m;
    let mut min = sea;
    let mut max = sea;
    for sample in samples {
        min = min.min(sample.elevation_m);
        max = max.max(sample.elevation_m);
    }
    let pad = ((max - min) * 0.08).max(50.0);
    let min = min - pad;
    let max = max + pad;
    let span = (max - min).max(1.0);
    let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
    let to_y = |z: f64| -> usize {
        (((max - z) / span) * (HEIGHT - 1) as f64)
            .round()
            .clamp(0.0, (HEIGHT - 1) as f64) as usize
    };
    let sea_y = to_y(sea);
    // Sky gradient.
    for y in 0..HEIGHT {
        let t = y as f64 / (HEIGHT - 1) as f64;
        let sky = [
            (36.0 + 40.0 * t) as u8,
            (48.0 + 46.0 * t) as u8,
            (66.0 + 54.0 * t) as u8,
        ];
        for x in 0..WIDTH {
            let at = (y * WIDTH + x) * 3;
            rgb[at..at + 3].copy_from_slice(&sky);
        }
    }
    // Terrain and water columns: water fills the basin between the sea line
    // and a submerged surface; rock fills everything below the surface.
    let n = samples.len().max(2);
    for x in 0..WIDTH {
        let t = x as f64 / (WIDTH - 1) as f64;
        let at_sample = ((t * (n - 1) as f64).round() as usize).min(n - 1);
        let sample = &samples[at_sample];
        let surface = to_y(sample.elevation_m);
        let submerged = surface > sea_y;
        for y in 0..HEIGHT {
            let at = (y * WIDTH + x) * 3;
            if y < surface {
                if submerged && y >= sea_y {
                    // Water column: lighter near the sea surface.
                    let depth = (y - sea_y) as f64 / (surface - sea_y).max(1) as f64;
                    let colour = [
                        (46.0 + 18.0 * (1.0 - depth)) as u8,
                        (96.0 + 30.0 * (1.0 - depth)) as u8,
                        (152.0 + 34.0 * (1.0 - depth)) as u8,
                    ];
                    rgb[at..at + 3].copy_from_slice(&colour);
                }
                continue;
            }
            let depth = (y - surface) as f64 / (HEIGHT - surface).max(1) as f64;
            let colour = [
                (120.0 + 50.0 * depth) as u8,
                (96.0 + 36.0 * depth) as u8,
                (70.0 + 24.0 * depth) as u8,
            ];
            rgb[at..at + 3].copy_from_slice(&colour);
        }
        // Surface line (lighter tone).
        if surface < HEIGHT {
            let at = (surface * WIDTH + x) * 3;
            rgb[at..at + 3].copy_from_slice(&[232, 224, 200]);
        }
        // Sea-level reference: dashed light line.
        if x % 16 < 8 {
            let at = (sea_y * WIDTH + x) * 3;
            rgb[at..at + 3].copy_from_slice(&[170, 200, 220]);
        }
    }
    encode_png_rgb(WIDTH as u32, HEIGHT as u32, &rgb)
}

/// Writes an 8-bit grayscale PNG file.
pub fn write_png_gray(path: &Path, width: u32, height: u32, pixels: &[u8]) -> GeoResult<()> {
    let bytes = encode_png_gray(width, height, pixels);
    write_bytes(path, &bytes)
}

/// Writes a PNG file.
pub fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) -> GeoResult<()> {
    let bytes = encode_png_rgb(width, height, rgb);
    write_bytes(path, &bytes)
}

/// Writes arbitrary bytes, creating parent directories.
pub fn write_bytes(path: &Path, bytes: &[u8]) -> GeoResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| GeoError::Io(format!("{}: {e}", parent.display())))?;
        }
    }
    let mut file =
        File::create(path).map_err(|e| GeoError::Io(format!("{}: {e}", path.display())))?;
    file.write_all(bytes)
        .map_err(|e| GeoError::Io(format!("{}: {e}", path.display())))
}

/// Builds an OBJ mesh of the terrain as a string.
#[must_use]
pub fn obj_string(world: &World, params: &Params, max_side: usize) -> String {
    let grid = world.grid;
    let step = ((grid.width().max(grid.height()) / max_side.max(2)) + 1).max(1);
    let cell_km = grid.cell_size_m() / 1000.0;
    let vertical_exaggeration = 1.0;
    let mut out = String::with_capacity(grid.cell_count() * 40);
    out.push_str("# GeoGenesis terrain export\n");
    out.push_str(&format!(
        "# grid {}x{}, cell {} km, sea level {} m\n",
        grid.width(),
        grid.height(),
        cell_km,
        world.sea_level_m
    ));
    out.push_str("o terrain\n");
    for y in (0..grid.height()).step_by(step) {
        for x in (0..grid.width()).step_by(step) {
            let z = world.elevation_m.get(x, y) * vertical_exaggeration;
            out.push_str(&format!(
                "v {:.3} {:.3} {:.3}\n",
                x as f64 * params.world.cell_km,
                z,
                y as f64 * params.world.cell_km
            ));
        }
    }
    let cols = grid.width().div_ceil(step);
    let rows = grid.height().div_ceil(step);
    for j in 0..rows.saturating_sub(1) {
        for i in 0..cols.saturating_sub(1) {
            let a = j * cols + i + 1;
            let b = a + 1;
            let c = a + cols;
            let d = c + 1;
            out.push_str(&format!("f {a} {c} {b}\n"));
            out.push_str(&format!("f {b} {c} {d}\n"));
        }
    }
    out
}

/// Encodes bytes as standard base64 (no line breaks).
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(triple >> 18) as usize & 63] as char);
        out.push(TABLE[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Encodes the terrain as a self-contained glTF 2.0 asset (`.gltf` with an
/// embedded base64 buffer): positions, normals, vertex colours and indices.
#[must_use]
pub fn gltf_string(world: &World, params: &Params, max_side: usize) -> String {
    let grid = world.grid;
    let step = ((grid.width().max(grid.height()) / max_side.max(2)) + 1).max(1);
    let columns = grid.width().div_ceil(step);
    let rows = grid.height().div_ceil(step);
    let count = columns * rows;
    let cell_m = grid.cell_size_m();
    let sea = world.sea_level_m;
    let mut positions = Vec::<f32>::with_capacity(count * 3);
    let mut normals = Vec::<f32>::with_capacity(count * 3);
    let mut colours = Vec::<f32>::with_capacity(count * 3);
    for row in 0..rows {
        for column in 0..columns {
            let x = (column * step).min(grid.width() - 1);
            let y = (row * step).min(grid.height() - 1);
            let z = world.elevation_m.get(x, y);
            positions.push(x as f32 * cell_m as f32);
            positions.push(z as f32);
            positions.push(y as f32 * cell_m as f32);
            let left = world.elevation_m.get(grid.clamp_x(x as isize - 1), y);
            let right = world.elevation_m.get(grid.clamp_x(x as isize + 1), y);
            let down = world.elevation_m.get(x, grid.clamp_y(y as isize - 1));
            let up = world.elevation_m.get(x, grid.clamp_y(y as isize + 1));
            let gx = ((right - left) / (2.0 * cell_m)) as f32;
            let gy = ((up - down) / (2.0 * cell_m)) as f32;
            let inv = 1.0 / (gx * gx + gy * gy + 1.0).sqrt();
            normals.push(-gx * inv);
            normals.push(inv);
            normals.push(-gy * inv);
            let land = ((z - sea) / (params.world.vertical_m.max(1.0))).clamp(0.0, 1.0);
            let (r, g, b) = if z <= sea {
                (0.12f32, 0.28f32, 0.45f32)
            } else {
                (
                    (0.30 + 0.62 * land) as f32,
                    (0.42 + 0.48 * land) as f32,
                    (0.26 + 0.62 * land) as f32,
                )
            };
            colours.push(r);
            colours.push(g);
            colours.push(b);
        }
    }
    let mut indices = Vec::<u32>::with_capacity((columns - 1) * (rows - 1) * 6);
    for row in 0..rows.saturating_sub(1) {
        for column in 0..columns.saturating_sub(1) {
            let a = (row * columns + column) as u32;
            let b = a + 1;
            let c = a + columns as u32;
            let d = c + 1;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    let mut buffer = Vec::<u8>::with_capacity(
        positions.len() * 4 + normals.len() * 4 + colours.len() * 4 + indices.len() * 4,
    );
    for value in &positions {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    let normal_offset = buffer.len();
    for value in &normals {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    let colour_offset = buffer.len();
    for value in &colours {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    let index_offset = buffer.len();
    for value in &indices {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    let min = [
        positions
            .iter()
            .step_by(3)
            .copied()
            .fold(f32::INFINITY, f32::min),
        positions
            .iter()
            .skip(1)
            .step_by(3)
            .copied()
            .fold(f32::INFINITY, f32::min),
        positions
            .iter()
            .skip(2)
            .step_by(3)
            .copied()
            .fold(f32::INFINITY, f32::min),
    ];
    let max = [
        positions
            .iter()
            .step_by(3)
            .copied()
            .fold(f32::NEG_INFINITY, f32::max),
        positions
            .iter()
            .skip(1)
            .step_by(3)
            .copied()
            .fold(f32::NEG_INFINITY, f32::max),
        positions
            .iter()
            .skip(2)
            .step_by(3)
            .copied()
            .fold(f32::NEG_INFINITY, f32::max),
    ];
    let buffer_len = buffer.len();
    let payload = base64_encode(&buffer);
    let position_bytes = positions.len() * 4;
    let normal_bytes = normals.len() * 4;
    let colour_bytes = colours.len() * 4;
    let index_bytes = indices.len() * 4;
    format!(
        r#"{{"asset":{{"version":"2.0","generator":"GeoGenesis"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0,"name":"terrain"}}],"meshes":[{{"name":"terrain","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"COLOR_0":2}},"indices":3,"mode":4}}]}}],"buffers":[{{"byteLength":{buffer_len},"uri":"data:application/octet-stream;base64,{payload}"}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{position_bytes},"target":34962}},{{"buffer":0,"byteOffset":{normal_offset},"byteLength":{normal_bytes},"target":34962}},{{"buffer":0,"byteOffset":{colour_offset},"byteLength":{colour_bytes},"target":34962}},{{"buffer":0,"byteOffset":{index_offset},"byteLength":{index_bytes},"target":34963}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":{count},"type":"VEC3","min":[{min0},{min1},{min2}],"max":[{max0},{max1},{max2}]}},{{"bufferView":1,"componentType":5126,"count":{count},"type":"VEC3"}},{{"bufferView":2,"componentType":5126,"count":{count},"type":"VEC3"}},{{"bufferView":3,"componentType":5125,"count":{index_count},"type":"SCALAR"}}]}}"#,
        buffer_len = buffer_len,
        payload = payload,
        position_bytes = position_bytes,
        normal_offset = normal_offset,
        normal_bytes = normal_bytes,
        colour_offset = colour_offset,
        colour_bytes = colour_bytes,
        index_offset = index_offset,
        index_bytes = index_bytes,
        count = count,
        min0 = min[0],
        min1 = min[1],
        min2 = min[2],
        max0 = max[0],
        max1 = max[1],
        max2 = max[2],
        index_count = indices.len(),
    )
}

/// Writes an OBJ mesh of the terrain.
pub fn write_obj(path: &Path, world: &World, params: &Params, max_side: usize) -> GeoResult<()> {
    let text = obj_string(world, params, max_side);
    write_bytes(path, text.as_bytes())
}

/// Builds an XYZ point cloud as a string (elevation in metres).
#[must_use]
pub fn xyz_string(world: &World, params: &Params, step: usize) -> String {
    let grid = world.grid;
    let step = step.max(1);
    let mut out = String::with_capacity(grid.cell_count() * 30);
    for y in (0..grid.height()).step_by(step) {
        for x in (0..grid.width()).step_by(step) {
            out.push_str(&format!(
                "{:.3} {:.3} {:.3}\n",
                x as f64 * params.world.cell_km,
                y as f64 * params.world.cell_km,
                world.elevation_m.get(x, y)
            ));
        }
    }
    out
}

/// Writes an XYZ point cloud.
pub fn write_xyz(path: &Path, world: &World, params: &Params, step: usize) -> GeoResult<()> {
    let text = xyz_string(world, params, step);
    write_bytes(path, text.as_bytes())
}

/// Writes a 32-bit `FLOAT` OpenEXR height texture of the world.
///
/// The single `Y` channel stores elevation in metres relative to the vertical
/// datum (sea level as-is), matching [`elevation_raw_f32`] and the GIS
/// exports. `Y` is OpenEXR's luminance channel, so standard image tools show
/// the file as a grayscale height map; call [`exr::exr_channels`] with
/// `ELEVATION` when a descriptive channel name is preferred.
///
/// # Errors
///
/// Propagates [`crate::error::GeoError::Io`] when the file cannot be written;
/// dimension and sample-count problems are reported by [`exr::exr_heightmap`].
pub fn exr_height_texture(world: &World, path: &Path) -> GeoResult<()> {
    let grid = world.grid;
    let values: Vec<f32> = world
        .elevation_m
        .as_slice()
        .iter()
        .map(|&elevation| elevation as f32)
        .collect();
    exr::exr_heightmap(path, grid.width() as u32, grid.height() as u32, &values)
}

/// Writes the world, and optional climate and hydrology snapshots, as a
/// NetCDF-3 classic file.
///
/// The grid uses the dimensions `y` (rows, slowest) and `x` (columns, fastest),
/// so `elevation_m[y * width + x]`. Variables are sorted alphabetically:
///
/// - `elevation_m` (`NC_FLOAT`, metres) — always written;
/// - `temperature_c` (`NC_FLOAT`, °C) and `precipitation_mm` (`NC_FLOAT`,
///   mm/yr) — when `climate` is given;
/// - `discharge_m3_yr` (`NC_FLOAT`, m³/yr) — when `hydro` is given.
///
/// Sea level is metadata, not a grid field, so it is stored as the global text
/// attribute `sea_level_m` (metres, three decimals) next to `title`.
///
/// # Errors
///
/// Returns [`crate::error::GeoError::InvalidConfig`] when a snapshot was
/// computed on a grid with different dimensions, and propagates the writer's
/// errors otherwise.
pub fn netcdf_world(
    path: &Path,
    world: &World,
    climate: Option<&Climate>,
    hydro: Option<&Hydro>,
) -> GeoResult<()> {
    let grid = world.grid;
    if let Some(climate) = climate {
        if climate.grid.width() != grid.width() || climate.grid.height() != grid.height() {
            return Err(GeoError::InvalidConfig(
                "climate snapshot grid does not match the world grid".to_string(),
            ));
        }
    }
    if let Some(hydro) = hydro {
        if hydro.grid.width() != grid.width() || hydro.grid.height() != grid.height() {
            return Err(GeoError::InvalidConfig(
                "hydrology snapshot grid does not match the world grid".to_string(),
            ));
        }
    }
    let elevation: Vec<f32> = world
        .elevation_m
        .as_slice()
        .iter()
        .map(|&elevation| elevation as f32)
        .collect();
    let temperature: Option<Vec<f32>> =
        climate.map(|climate| climate.temperature_c.as_slice().to_vec());
    let precipitation: Option<Vec<f32>> =
        climate.map(|climate| climate.precipitation_mm.as_slice().to_vec());
    let discharge: Option<Vec<f32>> = hydro.map(|hydro| {
        hydro
            .discharge_m3_yr
            .as_slice()
            .iter()
            .map(|&flow| flow as f32)
            .collect()
    });

    let mut fields: Vec<netcdf::NetCdfField<'_>> = Vec::with_capacity(4);
    fields.push(netcdf::NetCdfField {
        name: "elevation_m",
        units: Some("m"),
        values: &elevation,
    });
    if let Some(values) = temperature.as_deref() {
        fields.push(netcdf::NetCdfField {
            name: "temperature_c",
            units: Some("degC"),
            values,
        });
    }
    if let Some(values) = precipitation.as_deref() {
        fields.push(netcdf::NetCdfField {
            name: "precipitation_mm",
            units: Some("mm/yr"),
            values,
        });
    }
    if let Some(values) = discharge.as_deref() {
        fields.push(netcdf::NetCdfField {
            name: "discharge_m3_yr",
            units: Some("m^3/yr"),
            values,
        });
    }
    let sea_level = format!("{:.3}", world.sea_level_m);
    netcdf::write_netcdf_fields_with_attrs(
        path,
        grid.width() as u32,
        grid.height() as u32,
        &[
            ("title", "GeoGenesis world export"),
            ("sea_level_m", sea_level.as_str()),
        ],
        &fields,
    )
}

/// Writes every requested export for a world into `dir`.
pub fn write_all(
    dir: &Path,
    world: &World,
    params: &Params,
    climate: Option<&Climate>,
    geology: Option<&crate::geology::Geology>,
    hydro: Option<&Hydro>,
) -> GeoResult<Vec<String>> {
    std::fs::create_dir_all(dir).map_err(|e| GeoError::Io(format!("{}: {e}", dir.display())))?;
    let mut written = Vec::new();
    let grid = world.grid;
    let rgb = colour_map(world, climate, geology, hydro, MapColour::Terrain);
    let png = encode_png_rgb(grid.width() as u32, grid.height() as u32, &rgb);
    write_bytes(&dir.join("terrain.png"), &png)?;
    written.push("terrain.png".to_string());
    let biome = colour_map(world, climate, geology, hydro, MapColour::Biomes);
    let png = encode_png_rgb(grid.width() as u32, grid.height() as u32, &biome);
    write_bytes(&dir.join("biomes.png"), &png)?;
    written.push("biomes.png".to_string());
    let rivers = colour_map(world, climate, geology, hydro, MapColour::Rivers);
    let png = encode_png_rgb(grid.width() as u32, grid.height() as u32, &rivers);
    write_bytes(&dir.join("rivers.png"), &png)?;
    written.push("rivers.png".to_string());
    write_bytes(&dir.join("elevation.f32"), &elevation_raw_f32(world))?;
    written.push("elevation.f32".to_string());
    write_bytes(&dir.join("height16.png"), &height_png16(world))?;
    written.push("height16.png".to_string());
    write_bytes(&dir.join("normals.png"), &normal_map_png(world))?;
    written.push("normals.png".to_string());
    if let (Some(climate), Some(geology)) = (climate, geology) {
        let w = grid.width() as f64;
        let h = grid.height() as f64;
        let samples = crate::stats::profile(
            world,
            climate,
            geology,
            w * 0.15,
            h * 0.5,
            w * 0.85,
            h * 0.5,
            768,
        );
        write_bytes(&dir.join("profile.png"), &profile_png(world, &samples))?;
        written.push("profile.png".to_string());
    }
    write_obj(&dir.join("terrain.obj"), world, params, 512)?;
    written.push("terrain.obj".to_string());
    write_bytes(
        &dir.join("terrain.gltf"),
        gltf_string(world, params, 512).as_bytes(),
    )?;
    written.push("terrain.gltf".to_string());
    write_xyz(&dir.join("terrain.xyz"), world, params, 2)?;
    written.push("terrain.xyz".to_string());
    exr_height_texture(world, &dir.join("terrain.exr"))?;
    written.push("terrain.exr".to_string());
    netcdf_world(&dir.join("fields.nc"), world, climate, hydro)?;
    written.push("fields.nc".to_string());
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;
    use crate::world::World;

    fn world() -> (World, Params) {
        let mut p = Params::default();
        p.world.width = 16;
        p.world.height = 16;
        p.world.cell_km = 5.0;
        let mut w = World::new(&p.world).unwrap();
        w.elevation_m
            .map_in_place(|i, _| ((i % 16) as f64) * 100.0 - 500.0);
        (w, p)
    }

    fn temp_dir(test: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("geogenesis_export_{}_{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn read_be_u32(bytes: &[u8], pos: &mut usize) -> u32 {
        let value = u32::from_be_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        value
    }

    fn read_cdf_name(bytes: &[u8], pos: &mut usize) -> String {
        let len = read_be_u32(bytes, pos) as usize;
        let name = String::from_utf8(bytes[*pos..*pos + len].to_vec()).unwrap();
        *pos += (len + 3) & !3;
        name
    }

    /// Parses an attribute list, returning the `(name, raw value)` pairs.
    fn read_cdf_attrs(bytes: &[u8], pos: &mut usize) -> Vec<(String, Vec<u8>)> {
        let tag = read_be_u32(bytes, pos);
        let count = read_be_u32(bytes, pos);
        if tag == 0 {
            assert_eq!(count, 0, "absent attribute list");
            return Vec::new();
        }
        assert_eq!(tag, 12, "NC_ATTRIBUTE tag");
        let mut attrs = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = read_cdf_name(bytes, pos);
            let nc_type = read_be_u32(bytes, pos);
            let nelems = read_be_u32(bytes, pos) as usize;
            let size = match nc_type {
                1 | 2 => nelems,     // NC_BYTE, NC_CHAR
                3 => nelems * 2,     // NC_SHORT
                4 | 5 => nelems * 4, // NC_INT, NC_FLOAT
                6 => nelems * 8,     // NC_DOUBLE
                other => panic!("unexpected netCDF type {other}"),
            };
            let values = bytes[*pos..*pos + size].to_vec();
            *pos += (size + 3) & !3;
            attrs.push((name, values));
        }
        attrs
    }

    struct NcVar {
        name: String,
        nc_type: u32,
        begin: usize,
        attrs: Vec<(String, Vec<u8>)>,
    }

    struct NcHeader {
        dims: Vec<(String, u32)>,
        gatts: Vec<(String, Vec<u8>)>,
        vars: Vec<NcVar>,
    }

    /// Minimal CDF-1 header reader used to verify `fields.nc` structure.
    fn parse_cdf_header(bytes: &[u8]) -> NcHeader {
        assert_eq!(&bytes[..4], b"CDF\x01", "NetCDF magic");
        let mut pos = 4;
        let _numrecs = read_be_u32(bytes, &mut pos);
        assert_eq!(read_be_u32(bytes, &mut pos), 10, "NC_DIMENSION tag");
        let dim_count = read_be_u32(bytes, &mut pos) as usize;
        let mut dims = Vec::with_capacity(dim_count);
        for _ in 0..dim_count {
            let name = read_cdf_name(bytes, &mut pos);
            dims.push((name, read_be_u32(bytes, &mut pos)));
        }
        let gatts = read_cdf_attrs(bytes, &mut pos);
        let tag = read_be_u32(bytes, &mut pos);
        let var_count = read_be_u32(bytes, &mut pos) as usize;
        let mut vars = Vec::new();
        if tag != 0 {
            assert_eq!(tag, 11, "NC_VARIABLE tag");
            for _ in 0..var_count {
                let name = read_cdf_name(bytes, &mut pos);
                let rank = read_be_u32(bytes, &mut pos) as usize;
                for _ in 0..rank {
                    let _dimid = read_be_u32(bytes, &mut pos);
                }
                let attrs = read_cdf_attrs(bytes, &mut pos);
                let nc_type = read_be_u32(bytes, &mut pos);
                let _vsize = read_be_u32(bytes, &mut pos);
                let begin = read_be_u32(bytes, &mut pos) as usize;
                vars.push(NcVar {
                    name,
                    nc_type,
                    begin,
                    attrs,
                });
            }
        } else {
            assert_eq!(var_count, 0, "absent variable list");
        }
        NcHeader { dims, gatts, vars }
    }

    fn cdf_var<'a>(header: &'a NcHeader, name: &str) -> &'a NcVar {
        header
            .vars
            .iter()
            .find(|var| var.name == name)
            .unwrap_or_else(|| panic!("missing NetCDF variable `{name}`"))
    }

    fn cdf_units(var: &NcVar) -> Option<String> {
        var.attrs
            .iter()
            .find(|(name, _)| name == "units")
            .map(|(_, values)| String::from_utf8(values.clone()).unwrap())
    }

    fn cdf_f32_values(bytes: &[u8], var: &NcVar, count: usize) -> Vec<f32> {
        assert_eq!(var.nc_type, 5, "NC_FLOAT variable");
        let mut pos = var.begin;
        (0..count)
            .map(|_| f32::from_bits(read_be_u32(bytes, &mut pos)))
            .collect()
    }

    #[test]
    fn raw_export_has_the_right_size_and_precision() {
        let (w, _) = world();
        let bytes = elevation_raw_f32(&w);
        assert_eq!(bytes.len(), w.cell_count() * 4);
        let first = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert_eq!(f64::from(first), w.elevation_m.get(0, 0));
    }

    #[test]
    fn gltf_is_self_contained_and_consistent() {
        let (w, p) = world();
        let text = gltf_string(&w, &p, 16);
        assert!(text.starts_with("{\"asset\":{\"version\":\"2.0\""));
        assert!(text.contains("\"POSITION\":0"));
        assert!(text.contains("data:application/octet-stream;base64,"));
        let byte_length: usize = text
            .split("\"byteLength\":")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(byte_length > 0);
        // The embedded buffer decodes to exactly the declared byte length.
        let encoded = text.split("base64,").nth(1).unwrap();
        let encoded = &encoded[..encoded.find('"').unwrap()];
        assert_eq!(encoded.len() % 4, 0);
    }

    #[test]
    fn profile_png_has_valid_header() {
        let (w, _) = world();
        let samples: Vec<crate::stats::ProfileSample> = (0..64)
            .map(|k| crate::stats::ProfileSample {
                distance_m: k as f64 * 1000.0,
                elevation_m: -300.0 + (k as f64 * 12.0),
                water_depth_m: 0.0,
                rock: 0,
                biome: 0,
            })
            .collect();
        let png = profile_png(&w, &samples);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(
            u32::from_be_bytes([png[16], png[17], png[18], png[19]]),
            1024
        );
        assert_eq!(
            u32::from_be_bytes([png[20], png[21], png[22], png[23]]),
            320
        );
    }

    #[test]
    fn height16_and_normals_pngs_have_valid_headers() {
        let (w, _) = world();
        let height = height_png16(&w);
        assert_eq!(
            &height[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
        );
        // IHDR: width, height, bit depth 16, colour type 0.
        assert_eq!(height[8..12], 13u32.to_be_bytes());
        assert_eq!(&height[12..16], b"IHDR");
        assert_eq!(height[24], 16, "bit depth");
        assert_eq!(height[25], 0, "colour type");
        let normals = normal_map_png(&w);
        assert_eq!(normals[24], 8, "normal map bit depth");
        assert_eq!(normals[25], 2, "normal map colour type");
        // A flat-ish world should have normals mostly pointing out of the map.
        let rgb = &normals[41..];
        assert!(rgb.len() > 100);
    }

    #[test]
    fn png_has_a_valid_signature_and_chunks() {
        let (w, _) = world();
        let rgb = colour_map(&w, None, None, None, MapColour::Terrain);
        let png = encode_png_rgb(w.grid.width() as u32, w.grid.height() as u32, &rgb);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // IHDR length field is 13.
        assert_eq!(&png[8..12], &[0, 0, 0, 13]);
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert!(png.windows(4).any(|w| w == b"IEND"));
    }

    #[test]
    fn zlib_adler_checksum_is_correct() {
        // Adler-32 of "Wikipedia" is 0x11E60398.
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // CRC-32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn obj_export_lists_vertices_and_faces() {
        let (w, p) = world();
        let dir = std::env::temp_dir().join("geogenesis_obj_test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("t.obj");
        write_obj(&path, &w, &p, 8).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("v "));
        assert!(text.contains("f "));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_all_writes_exr_and_netcdf_exports() {
        let (w, p) = world();
        let dir = temp_dir("all_exports");
        let written = write_all(&dir, &w, &p, None, None, None).unwrap();
        let existing = [
            "terrain.png",
            "biomes.png",
            "rivers.png",
            "elevation.f32",
            "height16.png",
            "normals.png",
            "terrain.obj",
            "terrain.gltf",
            "terrain.xyz",
        ];
        for (index, name) in existing.iter().enumerate() {
            assert_eq!(
                written[index], *name,
                "existing export order must not change"
            );
        }
        assert_eq!(written.len(), existing.len() + 2);
        assert!(written.contains(&"terrain.exr".to_string()));
        assert!(written.contains(&"fields.nc".to_string()));

        // EXR: magic, version 2 (single-part scanline, no flags) and a chlist.
        let exr = std::fs::read(dir.join("terrain.exr")).unwrap();
        assert_eq!(&exr[..4], &[0x76, 0x2f, 0x31, 0x01], "EXR magic");
        assert_eq!(u32::from_le_bytes(exr[4..8].try_into().unwrap()), 2);
        assert!(exr.windows(6).any(|window| window == b"chlist"));

        // NetCDF: classic header, dimensions, variables and data.
        let nc = std::fs::read(dir.join("fields.nc")).unwrap();
        let header = parse_cdf_header(&nc);
        assert_eq!(
            header.dims,
            vec![("y".to_string(), 16), ("x".to_string(), 16)]
        );
        let names: Vec<&str> = header.vars.iter().map(|var| var.name.as_str()).collect();
        assert_eq!(names, vec!["elevation_m"]);
        assert_eq!(cdf_units(&header.vars[0]).as_deref(), Some("m"));
        assert_eq!(header.gatts[0].0, "title");
        assert_eq!(
            header.gatts[0].1.as_slice(),
            b"GeoGenesis world export".as_slice()
        );
        assert_eq!(header.gatts[1].0, "sea_level_m");
        let sea_level: f64 = std::str::from_utf8(&header.gatts[1].1)
            .unwrap()
            .parse()
            .unwrap();
        assert!((sea_level - w.sea_level_m).abs() <= 5e-4);
        let decoded = cdf_f32_values(&nc, &header.vars[0], w.cell_count());
        for (index, elevation) in w.elevation_m.as_slice().iter().enumerate() {
            assert_eq!(decoded[index].to_bits(), (*elevation as f32).to_bits());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn netcdf_world_includes_optional_snapshots() {
        let (w, p) = world();
        let dir = temp_dir("netcdf_snapshots");
        let climate = crate::climate::compute(&crate::climate::ClimateInputs {
            world: &w,
            params: &p,
            previous: None,
            dt_yr: 0.0,
        });
        let hydro = crate::hydrology::route(&w, &p, Some(&climate));

        let full_path = dir.join("fields.nc");
        netcdf_world(&full_path, &w, Some(&climate), Some(&hydro)).unwrap();
        let nc = std::fs::read(&full_path).unwrap();
        let header = parse_cdf_header(&nc);
        let names: Vec<&str> = header.vars.iter().map(|var| var.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "discharge_m3_yr",
                "elevation_m",
                "precipitation_mm",
                "temperature_c",
            ],
            "variables must be sorted alphabetically"
        );
        assert_eq!(
            cdf_units(cdf_var(&header, "elevation_m")).as_deref(),
            Some("m")
        );
        assert_eq!(
            cdf_units(cdf_var(&header, "temperature_c")).as_deref(),
            Some("degC")
        );
        assert_eq!(
            cdf_units(cdf_var(&header, "precipitation_mm")).as_deref(),
            Some("mm/yr")
        );
        assert_eq!(
            cdf_units(cdf_var(&header, "discharge_m3_yr")).as_deref(),
            Some("m^3/yr")
        );

        let temperature = cdf_f32_values(&nc, cdf_var(&header, "temperature_c"), w.cell_count());
        for (index, expected) in climate.temperature_c.as_slice().iter().enumerate() {
            assert_eq!(temperature[index].to_bits(), expected.to_bits());
        }
        let discharge = cdf_f32_values(&nc, cdf_var(&header, "discharge_m3_yr"), w.cell_count());
        for (index, expected) in hydro.discharge_m3_yr.as_slice().iter().enumerate() {
            assert_eq!(discharge[index].to_bits(), (*expected as f32).to_bits());
        }

        // Without snapshots the file still opens and has fewer variables.
        let minimal_path = dir.join("minimal.nc");
        netcdf_world(&minimal_path, &w, None, None).unwrap();
        let minimal = parse_cdf_header(&std::fs::read(&minimal_path).unwrap());
        assert_eq!(minimal.vars.len(), 1);
        assert_eq!(minimal.vars[0].name, "elevation_m");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn colour_maps_have_deterministic_sizes() {
        let (w, _) = world();
        for mode in [
            MapColour::Terrain,
            MapColour::Elevation,
            MapColour::Slope,
            MapColour::Biomes,
            MapColour::Rock,
        ] {
            let rgb = colour_map(&w, None, None, None, mode);
            assert_eq!(rgb.len(), w.cell_count() * 3);
        }
    }
}
