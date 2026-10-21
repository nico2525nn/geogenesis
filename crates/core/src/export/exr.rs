//! Dependency-free OpenEXR writer (single-part scanline images, version 2).
//!
//! The writer emits the canonical uncompressed form of the format described by
//! the [OpenEXR file layout](https://openexr.com/en/latest/OpenEXRFileLayout.html):
//!
//! ```text
//! magic (0x76 0x2f 0x31 0x01) | version field (2) | header | line offset table
//! per scanline: y (i32) | pixel data size (u32) | channel data
//! ```
//!
//! Every integer and float is little-endian. The header carries the eight
//! required attributes (`channels`, `compression`, `dataWindow`,
//! `displayWindow`, `lineOrder`, `pixelAspectRatio`, `screenWindowCenter`,
//! `screenWindowWidth`) sorted by name, exactly as the reference library emits
//! them. `compression` is `NO_COMPRESSION` (0), so every scanline block holds
//! one uncompressed scanline and readers never need a compression codec; this
//! is accepted by OpenEXR, ImageMagick, ffmpeg and other EXR readers.
//!
//! Channels are stored in alphabetical order, both in the `chlist` attribute
//! and inside each scanline's pixel data, as the format requires. `FLOAT`
//! channels keep the exact bits of each `f32` (including NaN payloads and
//! infinities); `HALF` channels round to the nearest binary16 value (ties to
//! even) and turn signalling NaNs into quiet NaNs. The data window is
//! `(0, 0)` to `(width - 1, height - 1)`, so the written image has no offsets.
//!
//! The encoder does not write optional metadata (`software`, `comments`); the
//! resulting header is byte-identical to the reference sample file in the
//! OpenEXR documentation for the matching image, which a unit test checks.
//!
//! Output is streamed scanline by scanline through a small per-row buffer, so
//! memory use is independent of the image height (only one row of encoded
//! channel data is held at a time).

use crate::error::{GeoError, GeoResult};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// OpenEXR magic number (`int` 20000630) as stored on disk.
const MAGIC: [u8; 4] = [0x76, 0x2f, 0x31, 0x01];

/// File format version stored in the low byte of the version field.
const VERSION: u32 = 2;

/// Version-field flag: the file uses names longer than 31 bytes.
const LONG_NAME_FLAG: u32 = 0x400;

/// `chlist` pixel type: 16-bit half float.
const PIXEL_TYPE_HALF: i32 = 1;

/// `chlist` pixel type: 32-bit float.
const PIXEL_TYPE_FLOAT: i32 = 2;

/// `compression` attribute value: no compression.
const NO_COMPRESSION: u8 = 0;

/// `lineOrder` attribute value: scanlines stored top to bottom.
const INCREASING_Y: u8 = 0;

/// Longest name allowed in a file: 31 bytes normally, 255 with the long-name flag.
const MAX_NAME_BYTES: usize = 255;

/// Sample type of an [`ExrChannel`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExrSampleType {
    /// 16-bit half float (2 bytes per sample).
    Half,
    /// 32-bit float (4 bytes per sample).
    Float,
}

impl ExrSampleType {
    /// `chlist` pixel type code.
    fn code(self) -> i32 {
        match self {
            Self::Half => PIXEL_TYPE_HALF,
            Self::Float => PIXEL_TYPE_FLOAT,
        }
    }

    /// Bytes consumed by one sample on disk.
    fn bytes_per_sample(self) -> u64 {
        match self {
            Self::Half => 2,
            Self::Float => 4,
        }
    }
}

/// One channel of a single-part scanline image.
///
/// `values` is row-major with the top scanline first: `values[y * width + x]`.
#[derive(Clone, Copy, Debug)]
pub struct ExrChannel<'a> {
    /// Channel name, 1-255 bytes; names are written in alphabetical order.
    pub name: &'a str,
    /// Sample type stored for this channel.
    pub sample_type: ExrSampleType,
    /// `width * height` samples for this channel.
    pub values: &'a [f32],
}

/// Encodes a height map as a one-channel `FLOAT` EXR with channel `Y`.
///
/// `Y` is OpenEXR's luminance channel, so standard image tools (ImageMagick,
/// ffmpeg, Blender, ...) load the file as a grayscale height map. Callers that
/// want a descriptive channel name can use [`exr_channels`] with `ELEVATION`.
///
/// # Errors
///
/// Returns [`GeoError::InvalidGrid`] for zero or oversized dimensions,
/// [`GeoError::InvalidConfig`] when `values` does not hold exactly
/// `width * height` samples, and [`GeoError::Io`] when the file cannot be
/// written.
pub fn exr_heightmap(path: &Path, width: u32, height: u32, values: &[f32]) -> GeoResult<()> {
    write_file(
        path,
        width,
        height,
        &[ExrChannel {
            name: "Y",
            sample_type: ExrSampleType::Float,
            values,
        }],
    )
}

/// Encodes a height map as a one-channel `HALF` EXR with channel `Y`.
///
/// Half precision keeps the file small (2 bytes per sample) and is the usual
/// format for HDR height maps; values are rounded to nearest binary16.
///
/// # Errors
///
/// Same as [`exr_heightmap`].
pub fn exr_heightmap_half(path: &Path, width: u32, height: u32, values: &[f32]) -> GeoResult<()> {
    write_file(
        path,
        width,
        height,
        &[ExrChannel {
            name: "Y",
            sample_type: ExrSampleType::Half,
            values,
        }],
    )
}

/// Writes named `FLOAT` channels as one EXR image.
///
/// `channels` is a list of `(name, values)` pairs; every slice must hold
/// `width * height` samples in row-major order. Channels are sorted by name in
/// the file.
///
/// # Errors
///
/// Same as [`exr_heightmap`], plus [`GeoError::InvalidConfig`] when the channel
/// list is empty, a name is empty, contains control characters, exceeds 255
/// bytes, or when two channels share a name.
pub fn exr_channels(
    path: &Path,
    width: u32,
    height: u32,
    channels: &[(&str, &[f32])],
) -> GeoResult<()> {
    let typed: Vec<ExrChannel<'_>> = channels
        .iter()
        .map(|&(name, values)| ExrChannel {
            name,
            sample_type: ExrSampleType::Float,
            values,
        })
        .collect();
    write_file(path, width, height, &typed)
}

/// Writes named `HALF` channels as one EXR image (see [`exr_channels`]).
///
/// # Errors
///
/// Same as [`exr_channels`].
pub fn exr_channels_half(
    path: &Path,
    width: u32,
    height: u32,
    channels: &[(&str, &[f32])],
) -> GeoResult<()> {
    let typed: Vec<ExrChannel<'_>> = channels
        .iter()
        .map(|&(name, values)| ExrChannel {
            name,
            sample_type: ExrSampleType::Half,
            values,
        })
        .collect();
    write_file(path, width, height, &typed)
}

/// Writes arbitrary typed channels as one EXR image.
///
/// This is the general entry point behind [`exr_heightmap`],
/// [`exr_heightmap_half`], [`exr_channels`] and [`exr_channels_half`]; it
/// allows mixing `FLOAT` and `HALF` channels in one image.
///
/// # Errors
///
/// See [`ExrChannel`] and [`exr_heightmap`].
pub fn write_exr(
    path: &Path,
    width: u32,
    height: u32,
    channels: &[ExrChannel<'_>],
) -> GeoResult<()> {
    write_file(path, width, height, channels)
}

/// Encodes a complete EXR image in memory.
///
/// The returned bytes are deterministic. This is convenient for tests, WASM
/// callers and callers that handle file writing themselves; prefer
/// [`write_exr`] for large images, because it streams to the file instead of
/// buffering the whole output in memory.
///
/// # Errors
///
/// Same as [`write_exr`].
pub fn encode_exr(width: u32, height: u32, channels: &[ExrChannel<'_>]) -> GeoResult<Vec<u8>> {
    let (layout, header) = prepare(width, height, channels)?;
    let block_bytes = u64::from(layout.line_bytes) + 8;
    let total = header.len() as u64 + u64::from(height) * (8 + block_bytes);
    let capacity = usize::try_from(total).map_err(|_| {
        GeoError::InvalidConfig("EXR image does not fit this platform's address space".to_string())
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    write_stream(
        &mut bytes,
        width,
        height,
        channels,
        &layout,
        &header,
        "EXR encode",
    )?;
    Ok(bytes)
}

/// Validated, ordered channel layout shared by the header and the data writer.
struct Layout {
    /// Indices into the caller's channel slice, sorted by channel name.
    order: Vec<usize>,
    /// Bytes in one scanline across all channels.
    line_bytes: u32,
    /// Whether some channel name is longer than 31 bytes.
    long_names: bool,
}

/// Writes a complete EXR file, creating parent directories as needed.
fn write_file(path: &Path, width: u32, height: u32, channels: &[ExrChannel<'_>]) -> GeoResult<()> {
    let (layout, header) = prepare(width, height, channels)?;
    let mut file = create_output(path)?;
    write_stream(
        &mut file,
        width,
        height,
        channels,
        &layout,
        &header,
        &path.display().to_string(),
    )?;
    file.flush()
        .map_err(|e| GeoError::Io(format!("{}: {e}", path.display())))
}

/// Creates the output file and its parent directories.
fn create_output(path: &Path) -> GeoResult<BufWriter<File>> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| GeoError::Io(format!("{}: {e}", parent.display())))?;
        }
    }
    let file = File::create(path).map_err(|e| GeoError::Io(format!("{}: {e}", path.display())))?;
    Ok(BufWriter::new(file))
}

/// Validates the image and builds the encoded header.
fn prepare(width: u32, height: u32, channels: &[ExrChannel<'_>]) -> GeoResult<(Layout, Vec<u8>)> {
    let layout = layout(width, height, channels)?;
    let header = header_bytes(width, height, channels, &layout);
    Ok((layout, header))
}

/// Validates dimensions, names, lengths and scanline sizes, and sorts channels.
fn layout(width: u32, height: u32, channels: &[ExrChannel<'_>]) -> GeoResult<Layout> {
    if width == 0 || height == 0 {
        return Err(GeoError::InvalidGrid(format!(
            "EXR image must be at least 1x1 (got {width}x{height})"
        )));
    }
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(GeoError::InvalidGrid(format!(
            "EXR data window must fit a 32-bit signed box (got {width}x{height})"
        )));
    }
    if channels.is_empty() {
        return Err(GeoError::InvalidConfig(
            "EXR image needs at least one channel".to_string(),
        ));
    }
    let pixels = u64::from(width) * u64::from(height);
    let mut long_names = false;
    for channel in channels {
        validate_name(channel.name, "channel")?;
        long_names |= channel.name.len() > 31;
        if channel.values.len() as u64 != pixels {
            return Err(GeoError::InvalidConfig(format!(
                "EXR channel `{}` has {} samples, expected {pixels} ({width}x{height})",
                channel.name,
                channel.values.len()
            )));
        }
    }

    let mut order: Vec<usize> = (0..channels.len()).collect();
    order.sort_by(|&a, &b| channels[a].name.cmp(channels[b].name));
    for pair in order.windows(2) {
        if channels[pair[0]].name == channels[pair[1]].name {
            return Err(GeoError::InvalidConfig(format!(
                "duplicate EXR channel `{}`",
                channels[pair[0]].name
            )));
        }
    }

    let bytes_per_pixel: u64 = order
        .iter()
        .map(|&index| channels[index].sample_type.bytes_per_sample())
        .sum();
    let line_bytes = u64::from(width) * bytes_per_pixel;
    if line_bytes > i32::MAX as u64 {
        return Err(GeoError::InvalidConfig(format!(
            "EXR scanline of {line_bytes} bytes exceeds the 2 GiB chunk limit"
        )));
    }
    Ok(Layout {
        order,
        line_bytes: line_bytes as u32,
        long_names,
    })
}

/// Rejects names that the format (or its readers) cannot represent.
fn validate_name(name: &str, kind: &str) -> GeoResult<()> {
    if name.is_empty() {
        return Err(GeoError::InvalidConfig(format!("EXR {kind} name is empty")));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(GeoError::InvalidConfig(format!(
            "EXR {kind} name of {} bytes exceeds the {MAX_NAME_BYTES}-byte limit",
            name.len()
        )));
    }
    if name
        .bytes()
        .any(|byte| byte == 0 || byte < 0x20 || byte == 0x7f)
    {
        return Err(GeoError::InvalidConfig(format!(
            "EXR {kind} name `{name}` contains a control character"
        )));
    }
    Ok(())
}

/// Encodes the magic number, version field, header attributes and their
/// terminator. Attribute order is alphabetical, matching the reference writer.
fn header_bytes(width: u32, height: u32, channels: &[ExrChannel<'_>], layout: &Layout) -> Vec<u8> {
    let chlist = chlist_payload(channels, layout);
    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(&MAGIC);
    let version = if layout.long_names {
        VERSION | LONG_NAME_FLAG
    } else {
        VERSION
    };
    out.extend_from_slice(&version.to_le_bytes());

    push_attribute(&mut out, "channels", "chlist", &chlist);
    push_attribute(&mut out, "compression", "compression", &[NO_COMPRESSION]);
    push_attribute(&mut out, "dataWindow", "box2i", &box2i(width, height));
    push_attribute(&mut out, "displayWindow", "box2i", &box2i(width, height));
    push_attribute(&mut out, "lineOrder", "lineOrder", &[INCREASING_Y]);
    push_attribute(&mut out, "pixelAspectRatio", "float", &1.0f32.to_le_bytes());
    push_attribute(&mut out, "screenWindowCenter", "v2f", &[0u8; 8]);
    push_attribute(
        &mut out,
        "screenWindowWidth",
        "float",
        &1.0f32.to_le_bytes(),
    );
    out.push(0); // end of the single-part header
    out
}

/// Builds the `chlist` attribute payload.
fn chlist_payload(channels: &[ExrChannel<'_>], layout: &Layout) -> Vec<u8> {
    let mut payload = Vec::new();
    for &index in &layout.order {
        let channel = &channels[index];
        payload.extend_from_slice(channel.name.as_bytes());
        payload.push(0);
        payload.extend_from_slice(&channel.sample_type.code().to_le_bytes());
        payload.extend_from_slice(&[0, 0, 0, 0]); // pLinear, then three reserved bytes
        payload.extend_from_slice(&1i32.to_le_bytes()); // xSampling
        payload.extend_from_slice(&1i32.to_le_bytes()); // ySampling
    }
    payload.push(0); // end of channel list
    payload
}

/// Encodes a `box2i` covering `(0, 0)` to `(width - 1, height - 1)`.
fn box2i(width: u32, height: u32) -> [u8; 16] {
    let mut payload = [0u8; 16];
    payload[8..12].copy_from_slice(&(width as i32 - 1).to_le_bytes());
    payload[12..16].copy_from_slice(&(height as i32 - 1).to_le_bytes());
    payload
}

/// Appends one null-terminated-name attribute to the header.
fn push_attribute(out: &mut Vec<u8>, name: &str, kind: &str, payload: &[u8]) {
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(kind.as_bytes());
    out.push(0);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
}

/// Streams the offset table and the scanline blocks to `writer`.
fn write_stream<W: Write>(
    writer: &mut W,
    width: u32,
    height: u32,
    channels: &[ExrChannel<'_>],
    layout: &Layout,
    header: &[u8],
    context: &str,
) -> GeoResult<()> {
    let io = |error: std::io::Error| GeoError::Io(format!("{context}: {error}"));
    writer.write_all(header).map_err(io)?;

    // One offset per scanline block; a block is the y coordinate, the data size
    // and the uncompressed pixel data for that scanline.
    let block_bytes = u64::from(layout.line_bytes) + 8;
    let first = header.len() as u64 + u64::from(height) * 8;
    for line in 0..u64::from(height) {
        writer
            .write_all(&(first + line * block_bytes).to_le_bytes())
            .map_err(io)?;
    }

    let columns = width as usize;
    let mut row = Vec::with_capacity(layout.line_bytes as usize);
    for y in 0..height as usize {
        let start = y * columns;
        let end = start + columns;
        row.clear();
        for &index in &layout.order {
            let channel = &channels[index];
            for &value in &channel.values[start..end] {
                match channel.sample_type {
                    ExrSampleType::Float => row.extend_from_slice(&value.to_le_bytes()),
                    ExrSampleType::Half => {
                        row.extend_from_slice(&f32_to_f16(value).to_le_bytes());
                    }
                }
            }
        }
        debug_assert_eq!(row.len(), layout.line_bytes as usize);
        writer.write_all(&(y as i32).to_le_bytes()).map_err(io)?;
        writer
            .write_all(&layout.line_bytes.to_le_bytes())
            .map_err(io)?;
        writer.write_all(&row).map_err(io)?;
    }
    Ok(())
}

/// Converts an `f32` to the nearest binary16 bit pattern (round to nearest,
/// ties to even), preserving infinities and mapping NaNs to quiet NaNs.
///
/// The conversion is written out rather than pulled from a crate so the core
/// keeps zero dependencies and the WASM module stays free of float-conversion
/// libraries.
fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = (bits >> 23) & 0xff;
    let mantissa = bits & 0x007f_ffff;

    if exponent == 0xff {
        if mantissa == 0 {
            return sign | 0x7c00; // infinity
        }
        // Keep the most significant payload bits and force the quiet bit.
        let payload = ((mantissa >> 13) as u16) | 0x0200;
        return sign | 0x7c00 | payload;
    }
    if exponent == 0 {
        // f32 subnormals are billions of times smaller than the smallest half
        // subnormal, so they round to signed zero.
        return sign;
    }

    let half_exponent = exponent as i32 - 112; // (exponent - 127) + 15
    if half_exponent >= 31 {
        return sign | 0x7c00; // overflow to infinity
    }
    if half_exponent <= 0 {
        // Half subnormal. With v = 2^23 + mantissa and shift = 126 - exponent,
        // the half mantissa is round(v / 2^shift) to nearest, ties to even.
        let shift = (14 - half_exponent) as u32;
        let rounded = round_shift_right_even((1u64 << 23) | u64::from(mantissa), shift);
        if rounded >= 1024 {
            return sign | 0x0400; // rounded up to the smallest normal half
        }
        return sign | rounded as u16;
    }

    let mut half_exponent = half_exponent as u16;
    let mut half_mantissa = mantissa >> 13;
    let dropped = mantissa & 0x1fff;
    if dropped > 0x1000 || (dropped == 0x1000 && (half_mantissa & 1) == 1) {
        half_mantissa += 1;
    }
    if half_mantissa == 0x0400 {
        half_mantissa = 0;
        half_exponent += 1;
    }
    if half_exponent >= 31 {
        return sign | 0x7c00;
    }
    sign | (half_exponent << 10) | half_mantissa as u16
}

/// Shifts `value` right by `shift`, rounding to nearest with ties to even.
fn round_shift_right_even(value: u64, shift: u32) -> u64 {
    debug_assert!((1..64).contains(&shift));
    let quotient = value >> shift;
    let remainder = value & ((1u64 << shift) - 1);
    let halfway = 1u64 << (shift - 1);
    if remainder > halfway || (remainder == halfway && quotient & 1 == 1) {
        quotient + 1
    } else {
        quotient
    }
}

/// Converts a binary16 bit pattern to `f32` (exact). Used by tests as the
/// writer's HALF counterpart; the crate only needs the forward direction.
#[cfg(test)]
fn f16_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exponent = u32::from((bits >> 10) & 0x1f);
    let mantissa = u32::from(bits & 0x03ff);
    let converted = if exponent == 0 {
        if mantissa == 0 {
            sign
        } else {
            // mantissa * 2^-24 is exactly representable in f32.
            (mantissa as f32 * f32::from_bits(0x3380_0000)).to_bits() | sign
        }
    } else if exponent == 31 {
        sign | 0x7f80_0000 | (mantissa << 13)
    } else {
        sign | ((exponent + 112) << 23) | (mantissa << 13)
    };
    f32::from_bits(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 415-byte sample file from the OpenEXR file-layout documentation
    /// (<https://openexr.com/en/latest/OpenEXRFileLayout.html>): a 4x3,
    /// single-part scanline image with a `HALF` `G` channel and a `FLOAT` `Z`
    /// channel, both uncompressed.
    const REFERENCE_EXR_HEX: &str = concat!(
        "762f3101020000006368616e6e656c730063686c697374002500000047000100000000000000010000000100",
        "00005a000200000000000000010000000100000000636f6d7072657373696f6e00636f6d7072657373696f6e",
        "0001000000006461746157696e646f7700626f78326900100000000000000000000000030000000200000064",
        "6973706c617957696e646f7700626f7832690010000000000000000000000003000000020000006c696e654f",
        "72646572006c696e654f72646572000100000000706978656c417370656374526174696f00666c6f61740004",
        "0000000000803f73637265656e57696e646f7743656e74657200763266000800000000000000000000007363",
        "7265656e57696e646f77576964746800666c6f617400040000000000803f003f010000000000005f01000000",
        "0000007f01000000000000000000001800000000005429d535e82d5c28813acfe1343e8b0bbb3d8974f93e01",
        "0000001800000037387633743b73387fabe83e8acf543f5b6c113f2035503d0200000018000000233a0a3402",
        "3b5d3b38f39a3c4dad983e1c14083f4cf3033f",
    );

    fn hex_bytes(text: &str) -> Vec<u8> {
        assert_eq!(text.len() & 1, 0, "hex string has an even length");
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = (pair[0] as char).to_digit(16).expect("hex digit") as u8;
                let low = (pair[1] as char).to_digit(16).expect("hex digit") as u8;
                (high << 4) | low
            })
            .collect()
    }

    /// A decoded scanline.
    struct ParsedRow {
        y: i32,
        data_size: u32,
        /// Per-channel samples, in channel-list order.
        channels: Vec<Vec<f32>>,
    }

    /// Minimal reader used to verify the encoder against the format grammar.
    struct ParsedExr {
        version: u32,
        attrs: Vec<(String, String, Vec<u8>)>,
        channels: Vec<(String, i32)>,
        offsets: Vec<u64>,
        header_len: usize,
        width: usize,
        height: usize,
        rows: Vec<ParsedRow>,
    }

    impl ParsedExr {
        fn attr_entry(&self, name: &str) -> &(String, String, Vec<u8>) {
            self.attrs
                .iter()
                .find(|(attr_name, _, _)| attr_name == name)
                .unwrap_or_else(|| panic!("missing EXR attribute `{name}`"))
        }

        fn attr(&self, name: &str) -> &[u8] {
            &self.attr_entry(name).2
        }

        fn channel_index(&self, name: &str) -> usize {
            self.channels
                .iter()
                .position(|(channel_name, _)| channel_name == name)
                .unwrap_or_else(|| panic!("missing EXR channel `{name}`"))
        }

        /// Decoded samples of one channel, flattened row by row.
        fn channel_values(&self, name: &str) -> Vec<f32> {
            let index = self.channel_index(name);
            let mut values = Vec::with_capacity(self.width * self.height);
            for row in &self.rows {
                values.extend_from_slice(&row.channels[index]);
            }
            values
        }
    }

    fn read_u16(bytes: &[u8], pos: &mut usize) -> u16 {
        let value = u16::from_le_bytes(bytes[*pos..*pos + 2].try_into().unwrap());
        *pos += 2;
        value
    }

    fn read_u32(bytes: &[u8], pos: &mut usize) -> u32 {
        let value = u32::from_le_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        value
    }

    fn read_i32(bytes: &[u8], pos: &mut usize) -> i32 {
        read_u32(bytes, pos) as i32
    }

    fn read_u64(bytes: &[u8], pos: &mut usize) -> u64 {
        let value = u64::from_le_bytes(bytes[*pos..*pos + 8].try_into().unwrap());
        *pos += 8;
        value
    }

    fn read_cstr(bytes: &[u8], pos: &mut usize) -> String {
        let end = bytes[*pos..]
            .iter()
            .position(|&byte| byte == 0)
            .map(|offset| *pos + offset)
            .expect("unterminated string");
        let text = String::from_utf8(bytes[*pos..end].to_vec()).expect("UTF-8 name");
        *pos = end + 1;
        text
    }

    fn box2i_values(payload: &[u8]) -> [i32; 4] {
        assert_eq!(payload.len(), 16);
        let mut pos = 0;
        [
            read_i32(payload, &mut pos),
            read_i32(payload, &mut pos),
            read_i32(payload, &mut pos),
            read_i32(payload, &mut pos),
        ]
    }

    fn f32_value(payload: &[u8]) -> f32 {
        assert_eq!(payload.len(), 4);
        f32::from_bits(u32::from_le_bytes(payload.try_into().unwrap()))
    }

    fn parse_exr(bytes: &[u8]) -> ParsedExr {
        assert!(bytes.len() > 8);
        assert_eq!(&bytes[..4], &MAGIC, "magic number");
        let mut pos = 4;
        let version = read_u32(bytes, &mut pos);
        let mut attrs = Vec::new();
        loop {
            if bytes[pos] == 0 {
                pos += 1;
                break;
            }
            let name = read_cstr(bytes, &mut pos);
            let kind = read_cstr(bytes, &mut pos);
            let size = read_u32(bytes, &mut pos) as usize;
            let payload = bytes[pos..pos + size].to_vec();
            pos += size;
            attrs.push((name, kind, payload));
        }
        let header_len = pos;

        // Channel list.
        let chlist = attrs
            .iter()
            .find(|(name, _, _)| name == "channels")
            .expect("channels attribute")
            .2
            .clone();
        let mut channels = Vec::new();
        let mut channel_pos = 0;
        while chlist[channel_pos] != 0 {
            let name = read_cstr(&chlist, &mut channel_pos);
            let code = read_i32(&chlist, &mut channel_pos);
            assert_eq!(chlist[channel_pos], 0, "pLinear");
            channel_pos += 1;
            assert_eq!(
                &chlist[channel_pos..channel_pos + 3],
                &[0, 0, 0],
                "reserved"
            );
            channel_pos += 3;
            let x_sampling = read_i32(&chlist, &mut channel_pos);
            let y_sampling = read_i32(&chlist, &mut channel_pos);
            assert_eq!((x_sampling, y_sampling), (1, 1), "sampling factors");
            channels.push((name, code));
        }
        channel_pos += 1;
        assert_eq!(channel_pos, chlist.len(), "chlist payload length");

        // Data window drives the geometry and the offset-table length.
        let window = box2i_values(
            attrs
                .iter()
                .find(|(name, _, _)| name == "dataWindow")
                .expect("dataWindow attribute")
                .2
                .as_slice(),
        );
        assert_eq!((window[0], window[1]), (0, 0), "dataWindow origin");
        let width = (window[2] - window[0] + 1) as usize;
        let height = (window[3] - window[1] + 1) as usize;
        assert!(width > 0 && height > 0);

        let mut offsets = Vec::with_capacity(height);
        for _ in 0..height {
            offsets.push(read_u64(bytes, &mut pos));
        }
        let data_start = pos;

        let mut rows = Vec::with_capacity(height);
        for (line, &offset) in offsets.iter().enumerate() {
            let offset = usize::try_from(offset).expect("offset fits usize");
            assert!(
                offset >= data_start && offset < bytes.len(),
                "scanline {line} offset {offset} outside the data section"
            );
            let mut row_pos = offset;
            let y = read_i32(bytes, &mut row_pos);
            assert_eq!(y, window[1] + line as i32, "scanline y coordinate");
            let data_size = read_u32(bytes, &mut row_pos);
            let bytes_per_sample: usize = channels
                .iter()
                .map(|(_, code)| match *code {
                    PIXEL_TYPE_HALF => 2,
                    PIXEL_TYPE_FLOAT => 4,
                    other => panic!("unexpected pixel type {other}"),
                })
                .sum();
            assert_eq!(data_size as usize, width * bytes_per_sample);
            let pixel_data = &bytes[row_pos..row_pos + data_size as usize];
            let mut pixel_pos = 0;
            let mut decoded = Vec::with_capacity(channels.len());
            for (_, code) in &channels {
                let mut samples = Vec::with_capacity(width);
                for _ in 0..width {
                    samples.push(match *code {
                        PIXEL_TYPE_HALF => f16_to_f32(read_u16(pixel_data, &mut pixel_pos)),
                        PIXEL_TYPE_FLOAT => f32::from_bits(read_u32(pixel_data, &mut pixel_pos)),
                        other => panic!("unexpected pixel type {other}"),
                    });
                }
                decoded.push(samples);
            }
            assert_eq!(pixel_pos, pixel_data.len(), "scanline data consumed");
            rows.push(ParsedRow {
                y,
                data_size,
                channels: decoded,
            });
        }

        // Offset table and blocks must tile the rest of the file exactly.
        assert_eq!(
            offsets.first().copied(),
            Some(data_start as u64),
            "first scanline offset"
        );
        for (line, &offset) in offsets.iter().enumerate() {
            let next = offsets.get(line + 1).copied().unwrap_or(bytes.len() as u64);
            assert_eq!(
                offset + 8 + u64::from(rows[line].data_size),
                next,
                "scanline {line} block length"
            );
        }

        ParsedExr {
            version,
            attrs,
            channels,
            offsets,
            header_len,
            width,
            height,
            rows,
        }
    }

    fn temp_path(test: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("geogenesis_exr_{}_{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join("out.exr")
    }

    #[test]
    fn reproduces_the_official_openexr_sample_file_byte_for_byte() {
        let expected = hex_bytes(REFERENCE_EXR_HEX);
        assert_eq!(expected.len(), 415);
        let reference = parse_exr(&expected);
        assert_eq!(reference.version, 2);
        assert_eq!(
            reference.channels,
            vec![
                ("G".to_string(), PIXEL_TYPE_HALF),
                ("Z".to_string(), PIXEL_TYPE_FLOAT),
            ]
        );

        // Re-encode the reference pixels through the public encoder.
        let green = reference.channel_values("G");
        let depth = reference.channel_values("Z");
        assert_eq!(green.len(), 4 * 3);
        let channels = [
            ExrChannel {
                name: "G",
                sample_type: ExrSampleType::Half,
                values: &green,
            },
            ExrChannel {
                name: "Z",
                sample_type: ExrSampleType::Float,
                values: &depth,
            },
        ];
        let encoded = encode_exr(4, 3, &channels).unwrap();
        assert_eq!(
            encoded, expected,
            "encoder output must match the reference OpenEXR file exactly"
        );
    }

    #[test]
    fn heightmap_round_trips_and_writes_standard_attributes() {
        let values: Vec<f32> = vec![
            0.0,
            -1.5,
            1024.25,
            f32::from_bits(0x7fc1_2345), // NaN with payload
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0,
            3.0e-41, // f32 subnormal
            1.0e30,
            -42.0,
            88.0,
            9.5,
            -0.25,
            2.0,
            7.0,
        ];
        let channel = ExrChannel {
            name: "ELEVATION",
            sample_type: ExrSampleType::Float,
            values: &values,
        };
        let parsed = parse_exr(&encode_exr(5, 3, &[channel]).unwrap());

        assert_eq!(parsed.version, VERSION);
        assert_eq!(parsed.width, 5);
        assert_eq!(parsed.height, 3);
        assert_eq!(parsed.attr_entry("channels").1, "chlist");
        assert_eq!(parsed.attr_entry("compression").1, "compression");
        assert_eq!(parsed.attr_entry("dataWindow").1, "box2i");
        assert_eq!(parsed.attr_entry("displayWindow").1, "box2i");
        assert_eq!(parsed.attr_entry("lineOrder").1, "lineOrder");
        assert_eq!(parsed.attr_entry("pixelAspectRatio").1, "float");
        assert_eq!(parsed.attr_entry("screenWindowCenter").1, "v2f");
        assert_eq!(parsed.attr_entry("screenWindowWidth").1, "float");
        assert_eq!(parsed.attr("compression"), &[NO_COMPRESSION]);
        assert_eq!(parsed.attr("lineOrder"), &[INCREASING_Y]);
        assert_eq!(box2i_values(parsed.attr("dataWindow")), [0, 0, 4, 2]);
        assert_eq!(box2i_values(parsed.attr("displayWindow")), [0, 0, 4, 2]);
        assert_eq!(
            f32_value(parsed.attr("pixelAspectRatio")).to_bits(),
            1.0f32.to_bits()
        );
        assert_eq!(
            f32_value(parsed.attr("screenWindowWidth")).to_bits(),
            1.0f32.to_bits()
        );
        assert_eq!(parsed.attr("screenWindowCenter"), &[0u8; 8]);
        assert_eq!(
            parsed.channels,
            vec![("ELEVATION".to_string(), PIXEL_TYPE_FLOAT)]
        );
        assert_eq!(parsed.offsets.len(), 3);
        assert_eq!(parsed.offsets[0], (parsed.header_len + 3 * 8) as u64);
        assert_eq!(parsed.rows[0].data_size, 5 * 4);
        for (line, row) in parsed.rows.iter().enumerate() {
            assert_eq!(row.y, line as i32);
        }

        // Pixel values round-trip bit-exactly, including NaN, infinities and
        // signed zero.
        let decoded = parsed.channel_values("ELEVATION");
        for (index, expected) in values.iter().enumerate() {
            assert_eq!(
                decoded[index].to_bits(),
                expected.to_bits(),
                "sample {index}"
            );
        }
    }

    #[test]
    fn multi_channel_images_sort_channels_and_keep_values() {
        let count = 3 * 5;
        let alpha: Vec<f32> = (0..count).map(|i| i as f32 * 0.5 - 2.0).collect();
        let depth: Vec<f32> = (0..count).map(|i| i as f32 * 1.25 + 0.1).collect();
        // Deliberately unsorted input: the file must sort by name.
        let channels = [
            ExrChannel {
                name: "Z",
                sample_type: ExrSampleType::Float,
                values: &depth,
            },
            ExrChannel {
                name: "A",
                sample_type: ExrSampleType::Half,
                values: &alpha,
            },
        ];
        let parsed = parse_exr(&encode_exr(3, 5, &channels).unwrap());
        assert_eq!(
            parsed.channels,
            vec![
                ("A".to_string(), PIXEL_TYPE_HALF),
                ("Z".to_string(), PIXEL_TYPE_FLOAT),
            ]
        );
        assert_eq!(parsed.rows.len(), 5);
        assert_eq!(parsed.rows[0].data_size, (3 * 2 + 3 * 4) as u32);
        assert_eq!(parsed.offsets[0], (parsed.header_len + 5 * 8) as u64);
        let decoded_alpha = parsed.channel_values("A");
        let decoded_depth = parsed.channel_values("Z");
        for index in 0..count {
            assert_eq!(decoded_alpha[index].to_bits(), alpha[index].to_bits());
            assert_eq!(decoded_depth[index].to_bits(), depth[index].to_bits());
        }
    }

    #[test]
    fn float_channels_use_four_bytes_per_sample() {
        let (width, height) = (5u32, 3u32);
        let channel_count = 3u32;
        let count = (width * height) as usize;
        let first: Vec<f32> = (0..count).map(|i| i as f32).collect();
        let second: Vec<f32> = (0..count).map(|i| i as f32 * 2.0).collect();
        let third: Vec<f32> = (0..count).map(|i| i as f32 * 3.0).collect();
        // Deliberately unsorted channel names.
        let channels = [
            ExrChannel {
                name: "C",
                sample_type: ExrSampleType::Float,
                values: &first,
            },
            ExrChannel {
                name: "A",
                sample_type: ExrSampleType::Float,
                values: &second,
            },
            ExrChannel {
                name: "B",
                sample_type: ExrSampleType::Float,
                values: &third,
            },
        ];
        let parsed = parse_exr(&encode_exr(width, height, &channels).unwrap());
        let names: Vec<&str> = parsed
            .channels
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, vec!["A", "B", "C"]);
        assert_eq!(
            parsed.offsets.len(),
            height as usize,
            "one line offset per scanline"
        );
        assert_eq!(parsed.rows[0].data_size, 4 * width * channel_count);
        for row in &parsed.rows {
            assert_eq!(row.data_size, 4 * width * channel_count);
            assert_eq!(row.channels.len(), channel_count as usize);
        }
        // A, B and C hold the second, third and first input slices.
        for (channel, expected) in [("A", &second), ("B", &third), ("C", &first)] {
            let decoded = parsed.channel_values(channel);
            for (index, value) in expected.iter().enumerate() {
                assert_eq!(
                    decoded[index].to_bits(),
                    value.to_bits(),
                    "{channel}[{index}]"
                );
            }
        }
    }

    #[test]
    fn extreme_aspect_ratios_and_single_pixel_images_round_trip() {
        for (width, height) in [(1u32, 1u32), (1, 7), (7, 1), (2, 3), (16, 1)] {
            let count = (width * height) as usize;
            let values: Vec<f32> = (0..count).map(|i| i as f32 * -3.5 + 1.0).collect();
            let parsed = parse_exr(
                &encode_exr(
                    width,
                    height,
                    &[ExrChannel {
                        name: "Y",
                        sample_type: ExrSampleType::Float,
                        values: &values,
                    }],
                )
                .unwrap(),
            );
            assert_eq!(parsed.width, width as usize);
            assert_eq!(parsed.height, height as usize);
            assert_eq!(parsed.rows.len(), height as usize);
            let decoded = parsed.channel_values("Y");
            for (index, expected) in values.iter().enumerate() {
                assert_eq!(decoded[index].to_bits(), expected.to_bits());
            }
        }
    }

    #[test]
    fn long_channel_names_set_the_long_name_version_flag() {
        let short = [1.0f32, 2.0];
        let parsed = parse_exr(
            &encode_exr(
                2,
                1,
                &[ExrChannel {
                    name: "Y",
                    sample_type: ExrSampleType::Half,
                    values: &short,
                }],
            )
            .unwrap(),
        );
        assert_eq!(parsed.version, VERSION, "short names leave the flag clear");

        let long_name = "a_very_long_channel_name_for_the_exr_writer";
        assert!(long_name.len() > 31);
        let parsed = parse_exr(
            &encode_exr(
                2,
                1,
                &[ExrChannel {
                    name: long_name,
                    sample_type: ExrSampleType::Half,
                    values: &short,
                }],
            )
            .unwrap(),
        );
        assert_eq!(parsed.version, VERSION | LONG_NAME_FLAG);
        assert_eq!(
            parsed.channels,
            vec![(long_name.to_string(), PIXEL_TYPE_HALF)]
        );
    }

    #[test]
    fn half_samples_are_rounded_to_the_nearest_binary16_value() {
        // Values that binary16 represents exactly must round-trip bit for bit.
        let exact: [(f32, u16); 8] = [
            (0.0, 0x0000),
            (-0.0, 0x8000),
            (1.0, 0x3c00),
            (-2.0, 0xc000),
            (0.5, 0x3800),
            (65504.0, 0x7bff),                     // largest finite half
            (1.0 + 1.0 / 1024.0, 0x3c01),          // exactly representable
            (f32::from_bits(0x3380_0000), 0x0001), // 2^-24, smallest subnormal
        ];
        for (input, expected) in exact {
            assert_eq!(f32_to_f16(input), expected, "encode of {input:?}");
            assert_eq!(f16_to_f32(expected), input, "decode of {expected:#06x}");
        }

        // Values that need rounding or overflow to infinity.
        let rounded: [(f32, u16, f32); 6] = [
            (65536.0, 0x7c00, f32::INFINITY), // overflows to infinity
            (-65536.0, 0xfc00, f32::NEG_INFINITY),
            (65519.0, 0x7bff, 65504.0), // rounds down to the largest half
            (1.0 + 1.0 / 2048.0, 0x3c00, 1.0), // halfway, ties to even
            (1.0 + 3.0 / 2048.0, 0x3c02, 1.0 + 2.0 / 1024.0), // ties to even
            (f32::from_bits(0x0000_0001), 0x0000, 0.0), // f32 subnormal → zero
        ];
        for (input, expected, decoded) in rounded {
            assert_eq!(
                f32_to_f16(input),
                expected,
                "f32 {input:?} ({:#010x})",
                input.to_bits()
            );
            assert_eq!(
                f16_to_f32(expected).to_bits(),
                decoded.to_bits(),
                "decode of {expected:#06x}"
            );
        }

        assert_eq!(f32_to_f16(f32::INFINITY), 0x7c00);
        assert_eq!(f32_to_f16(f32::NEG_INFINITY), 0xfc00);
        assert!(f16_to_f32(f32_to_f16(f32::NAN)).is_nan());
    }

    #[test]
    fn half_conversion_round_trips_every_representable_pattern() {
        for bits in 0..=u16::MAX {
            let decoded = f16_to_f32(bits);
            let encoded = f32_to_f16(decoded);
            let exponent = (bits >> 10) & 0x1f;
            let mantissa = bits & 0x03ff;
            let expected = if exponent == 0x1f && mantissa != 0 {
                // Signalling NaNs are quieted, as IEEE conversions do.
                bits | 0x0200
            } else {
                bits
            };
            assert_eq!(encoded, expected, "half bits {bits:#06x}");
        }
    }

    #[test]
    fn rejects_invalid_arguments() {
        let empty: [f32; 0] = [];
        let one = [1.0f32];

        let error = encode_exr(
            0,
            1,
            &[ExrChannel {
                name: "Y",
                sample_type: ExrSampleType::Float,
                values: &empty,
            }],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidGrid(_)), "{error:?}");

        let error = encode_exr(
            1,
            0,
            &[ExrChannel {
                name: "Y",
                sample_type: ExrSampleType::Float,
                values: &empty,
            }],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidGrid(_)), "{error:?}");

        let error = encode_exr(
            u32::MAX,
            1,
            &[ExrChannel {
                name: "Y",
                sample_type: ExrSampleType::Float,
                values: &empty,
            }],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidGrid(_)), "{error:?}");

        let error = encode_exr(1, 1, &[]).unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        let error = encode_exr(
            1,
            1,
            &[ExrChannel {
                name: "",
                sample_type: ExrSampleType::Float,
                values: &one,
            }],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        let error = encode_exr(
            1,
            1,
            &[ExrChannel {
                name: "bad\0name",
                sample_type: ExrSampleType::Float,
                values: &one,
            }],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        let error = encode_exr(
            1,
            1,
            &[ExrChannel {
                name: "bad\tname",
                sample_type: ExrSampleType::Float,
                values: &one,
            }],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        let too_long = "x".repeat(MAX_NAME_BYTES + 1);
        let error = encode_exr(
            1,
            1,
            &[ExrChannel {
                name: &too_long,
                sample_type: ExrSampleType::Float,
                values: &one,
            }],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        // A 255-byte name is the longest the format allows.
        let longest = "x".repeat(MAX_NAME_BYTES);
        encode_exr(
            1,
            1,
            &[ExrChannel {
                name: &longest,
                sample_type: ExrSampleType::Float,
                values: &one,
            }],
        )
        .unwrap();

        let error = encode_exr(
            2,
            1,
            &[ExrChannel {
                name: "Y",
                sample_type: ExrSampleType::Float,
                values: &one,
            }],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        let error = encode_exr(
            1,
            1,
            &[
                ExrChannel {
                    name: "Y",
                    sample_type: ExrSampleType::Float,
                    values: &one,
                },
                ExrChannel {
                    name: "Y",
                    sample_type: ExrSampleType::Half,
                    values: &one,
                },
            ],
        )
        .unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");
    }

    #[test]
    fn file_writers_create_directories_and_round_trip() {
        let path = temp_path("file_writers");
        let values: Vec<f32> = (0..6).map(|i| i as f32 * 3.0 - 1.0).collect();

        exr_heightmap(&path, 3, 2, &values).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let parsed = parse_exr(&bytes);
        assert_eq!(parsed.channels, vec![("Y".to_string(), PIXEL_TYPE_FLOAT)]);
        let decoded = parsed.channel_values("Y");
        for (index, expected) in values.iter().enumerate() {
            assert_eq!(decoded[index].to_bits(), expected.to_bits());
        }
        assert_eq!(
            bytes,
            encode_exr(
                3,
                2,
                &[ExrChannel {
                    name: "Y",
                    sample_type: ExrSampleType::Float,
                    values: &values,
                }],
            )
            .unwrap()
        );

        let half_path = temp_path("file_writers_half");
        let alpha = [1.0f32, 2.0, -3.0, 0.5];
        exr_channels_half(&half_path, 2, 2, &[("A", &alpha)]).unwrap();
        let parsed = parse_exr(&std::fs::read(&half_path).unwrap());
        assert_eq!(parsed.channels, vec![("A".to_string(), PIXEL_TYPE_HALF)]);
        assert_eq!(parsed.channel_values("A"), alpha);

        exr_heightmap_half(&path, 3, 2, &values).unwrap();
        let parsed = parse_exr(&std::fs::read(&path).unwrap());
        assert_eq!(parsed.channels, vec![("Y".to_string(), PIXEL_TYPE_HALF)]);
        assert_eq!(parsed.channel_values("Y"), values);

        let channels_path = temp_path("file_writers_channels");
        let red = [0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0];
        exr_channels(&channels_path, 3, 2, &[("B", &red), ("R", &red)]).unwrap();
        let parsed = parse_exr(&std::fs::read(&channels_path).unwrap());
        assert_eq!(
            parsed.channels,
            vec![
                ("B".to_string(), PIXEL_TYPE_FLOAT),
                ("R".to_string(), PIXEL_TYPE_FLOAT),
            ]
        );

        for path in [&path, &half_path, &channels_path] {
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    #[test]
    fn encoding_is_deterministic() {
        let values: Vec<f32> = (0..12).map(|i| (i as f32).sin() * 1000.0).collect();
        let channels = [
            ExrChannel {
                name: "B",
                sample_type: ExrSampleType::Half,
                values: &values,
            },
            ExrChannel {
                name: "A",
                sample_type: ExrSampleType::Float,
                values: &values,
            },
        ];
        let first = encode_exr(4, 3, &channels).unwrap();
        let second = encode_exr(4, 3, &channels).unwrap();
        assert_eq!(first, second);
    }
}
