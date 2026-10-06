//! Little-endian f64 grid dump with a small header.

use crate::grid::ScalarField;

/// Writes width, height (u32 LE) then row-major f64 LE.
pub fn write(path: &std::path::Path, field: &ScalarField) -> Result<(), crate::GeoError> {
    let mut bytes = Vec::with_capacity(8 + field.len() * 8);
    bytes.extend_from_slice(&(field.width() as u32).to_le_bytes());
    bytes.extend_from_slice(&(field.height() as u32).to_le_bytes());
    for &v in field.as_slice() {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, &bytes)?;
    Ok(())
}

/// Reads back a file written by [`write`].
pub fn read(path: &std::path::Path) -> Result<(u32, u32, Vec<f64>), crate::GeoError> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 8 {
        return Err(crate::GeoError::Io("raw file too short for header".into()));
    }
    let w = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let h = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let n = w as usize * h as usize;
    if bytes.len() != 8 + n * 8 {
        return Err(crate::GeoError::Io(format!(
            "raw file length {} != header {}x{}",
            bytes.len(),
            w,
            h
        )));
    }
    let mut out = Vec::with_capacity(n);
    for chunk in bytes[8..].chunks_exact(8) {
        out.push(f64::from_le_bytes(chunk.try_into().unwrap()));
    }
    Ok((w, h, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Boundary, BoundaryKind};

    #[test]
    fn round_trip_preserves_bits() {
        let b = Boundary::new(BoundaryKind::Rectangular, 8, 6);
        let mut f = ScalarField::zeros(b);
        for i in 0..b.cell_count() {
            f.set_index(i, (i as f64) * 1.5 - 100.0);
        }
        let path = std::env::temp_dir().join("geogenesis_raw_test.bin");
        write(&path, &f).unwrap();
        let (w, h, data) = read(&path).unwrap();
        assert_eq!((w, h), (8, 6));
        assert_eq!(data, f.as_slice().to_vec());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn short_file_is_rejected() {
        let path = std::env::temp_dir().join("geogenesis_raw_short.bin");
        std::fs::write(&path, [0u8; 4]).unwrap();
        assert!(read(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}