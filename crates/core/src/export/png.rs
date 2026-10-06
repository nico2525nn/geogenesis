//! 16-bit grayscale PNG heightmap writer (dependency-free).
//!
//! Implements just enough of PNG to write a valid image: 8-byte signature,
//! IHDR / IDAT / IEND chunks, stored (uncompressed) DEFLATE blocks, Adler-32
//! and CRC32 computed inline. Grayscale-16 rows are prefixed with filter byte 0
//! (None). Elevation maps linearly onto 0…65535.

use crate::grid::ScalarField;

/// Writes the field as a 16-bit grayscale PNG.
pub fn write_heightmap(
    path: &std::path::Path,
    field: &ScalarField,
) -> Result<(), crate::GeoError> {
    let (w, h) = (field.width(), field.height());
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(crate::GeoError::InvalidGrid(format!(
            "png size {w}×{h} out of range"
        )));
    }
    let (lo, hi) = (field.min(), field.max());
    let span = (hi - lo).max(f64::MIN_POSITIVE);

    // Scanlines: filter byte 0 + big-endian u16 per cell.
    let mut raw = Vec::with_capacity(h * (1 + w * 2));
    for y in 0..h {
        raw.push(0u8);
        for x in 0..w {
            let t = ((field.get(x, y) - lo) / span).clamp(0.0, 1.0);
            let v = (t * 65535.0).round() as u16;
            raw.extend_from_slice(&v.to_be_bytes());
        }
    }

    let mut png = Vec::with_capacity(raw.len() + 100);
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]); // signature

    // IHDR: width, height, bit depth 16, colour type 0 (grayscale).
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[16, 0, 0, 0, 0]);
    push_chunk(&mut png, b"IHDR", &ihdr);

    push_chunk(&mut png, b"IDAT", &deflate_stored(&raw));
    push_chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, &png)?;
    Ok(())
}

/// Appends a length + type + data + CRC chunk.
fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind.as_slice(), data].concat()).to_be_bytes());
}

/// zlib stream wrapping stored (type-00) DEFLATE blocks.
fn deflate_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01]; // zlib header: deflate, default window
    let mut rest = data;
    // Stored blocks carry at most 65535 bytes.
    while !rest.is_empty() {
        let n = rest.len().min(65535);
        let (block, tail) = rest.split_at(n);
        rest = tail;
        out.push(if rest.is_empty() { 1 } else { 0 }); // BFINAL + BTYPE=00
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&(!n as u16).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32 checksum (zlib trailer).
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(4096) {
        for &byte in chunk {
            a = (a + byte as u32) % MOD;
            b = (b + a) % MOD;
        }
    }
    (b << 16) | a
}

/// CRC32 (ISO 3309, reflected) over a PNG chunk.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Boundary, BoundaryKind};

    fn check_png(bytes: &[u8], w: u32, h: u32) {
        // Signature.
        assert_eq!(&bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        // First chunk is IHDR with the right dimensions.
        let len = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
        assert_eq!(len, 13);
        assert_eq!(&bytes[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), w);
        assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), h);
        assert_eq!(bytes[24], 16, "must be 16-bit");
        assert_eq!(bytes[25], 0, "must be grayscale");
        // Ends with an empty IEND.
        assert_eq!(&bytes[bytes.len() - 12..bytes.len() - 8], &[0, 0, 0, 0]);
        assert_eq!(&bytes[bytes.len() - 8..bytes.len() - 4], b"IEND");
    }

    #[test]
    fn writes_a_valid_png() {
        let b = Boundary::new(BoundaryKind::Rectangular, 16, 12);
        let mut f = ScalarField::zeros(b);
        for i in 0..b.cell_count() {
            f.set_index(i, i as f64);
        }
        let path = std::env::temp_dir().join("geogenesis_height_test.png");
        write_heightmap(&path, &f).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        check_png(&bytes, 16, 12);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn idat_decompresses_to_exact_scanlines() {
        // Hand-rolled zlib is the risky part: reimplement the stored-block
        // reader here and check byte equality, so a malformed stream fails
        // loudly instead of producing a corrupt-but-parseable image.
        let b = Boundary::new(BoundaryKind::Rectangular, 10, 6);
        let mut f = ScalarField::zeros(b);
        for i in 0..b.cell_count() {
            f.set_index(i, (i * 37 % 1000) as f64);
        }
        let path = std::env::temp_dir().join("geogenesis_height_rt.png");
        write_heightmap(&path, &f).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // Collect IDAT payloads.
        let mut pos = 8usize;
        let mut idat = Vec::new();
        while pos + 12 <= bytes.len() {
            let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let kind = &bytes[pos + 4..pos + 8];
            let data = &bytes[pos + 8..pos + 8 + len];
            if kind == b"IDAT" {
                idat.extend_from_slice(data);
            }
            if kind == b"IEND" {
                break;
            }
            pos += 12 + len;
        }
        assert_eq!(idat[0], 0x78);
        assert_eq!(idat[1], 0x01);
        // Parse stored blocks.
        let mut p = 2usize;
        let mut out = Vec::new();
        loop {
            let header = idat[p];
            p += 1;
            assert_eq!(header & 0b110, 0, "only stored blocks are written");
            let last = header & 1 == 1;
            let n = u16::from_le_bytes([idat[p], idat[p + 1]]) as usize;
            let nlen = u16::from_le_bytes([idat[p + 2], idat[p + 3]]) as usize;
            assert_eq!(n ^ 0xFFFF, nlen, "stored length complement wrong");
            p += 4;
            out.extend_from_slice(&idat[p..p + n]);
            p += n;
            if last {
                break;
            }
        }
        // Adler-32 trailer matches the decompressed bytes.
        let stored = u32::from_be_bytes([idat[p], idat[p + 1], idat[p + 2], idat[p + 3]]);
        assert_eq!(stored, adler32(&out));
        // Rebuild expected scanlines and compare exactly.
        let (lo, hi) = (f.min(), f.max());
        let span = (hi - lo).max(f64::MIN_POSITIVE);
        let mut expect = Vec::new();
        for y in 0..6 {
            expect.push(0u8);
            for x in 0..10 {
                let t = ((f.get(x, y) - lo) / span).clamp(0.0, 1.0);
                expect.extend_from_slice(&((t * 65535.0).round() as u16).to_be_bytes());
            }
        }
        assert_eq!(out, expect, "decompressed scanlines differ");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn flat_field_writes_without_error() {
        let b = Boundary::new(BoundaryKind::Rectangular, 8, 8);
        let f = ScalarField::filled(b, 42.0);
        let path = std::env::temp_dir().join("geogenesis_height_flat.png");
        write_heightmap(&path, &f).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        check_png(&bytes, 8, 8);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn crc32_matches_the_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn adler32_matches_the_known_vector() {
        // "Wikipedia" per Python's zlib.adler32.
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"a"), 0x0062_0062);
    }
}