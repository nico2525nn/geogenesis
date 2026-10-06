//! XYZ ASCII point cloud: `x y z` in metres, one vertex per line.

use crate::grid::ScalarField;

/// Writes the field as XYZ text.
pub fn write(
    path: &std::path::Path,
    field: &ScalarField,
    cell_size_m: f64,
) -> Result<(), crate::GeoError> {
    use std::fmt::Write as _;
    let b = *field.boundary();
    let mut text = String::with_capacity(field.len() * 24);
    for y in 0..b.height() {
        for x in 0..b.width() {
            let _ = writeln!(
                text,
                "{} {} {}",
                x as f64 * cell_size_m,
                y as f64 * cell_size_m,
                field.get(x, y)
            );
        }
    }
    std::fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{Boundary, BoundaryKind};

    #[test]
    fn xyz_has_one_line_per_cell() {
        let b = Boundary::new(BoundaryKind::Rectangular, 4, 3);
        let f = ScalarField::filled(b, 7.5);
        let path = std::env::temp_dir().join("geogenesis_xyz_test.txt");
        write(&path, &f, 10.0).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 12);
        assert!(text.lines().next().unwrap().ends_with("7.5"));
        std::fs::remove_file(&path).ok();
    }
}