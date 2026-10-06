//! Wavefront OBJ triangle mesh with vertex normals.

use crate::grid::ScalarField;

/// Writes the field as an OBJ mesh (1-based indices, `vn` normals included).
pub fn write(
    path: &std::path::Path,
    field: &ScalarField,
    cell_size_m: f64,
) -> Result<(), crate::GeoError> {
    use std::fmt::Write as _;
    let b = *field.boundary();
    let (w, h) = (b.width(), b.height());
    let mut text = String::with_capacity(field.len() * 40);
    let _ = writeln!(text, "# geogenesis terrain {}x{}", w, h);

    for y in 0..h {
        for x in 0..w {
            let _ = writeln!(
                text,
                "v {} {} {}",
                x as f64 * cell_size_m,
                y as f64 * cell_size_m,
                field.get(x, y)
            );
        }
    }
    // Analytic normals from the gradient: cheaper and smoother than averaging
    // face normals, and exact for the sampled surface.
    for y in 0..h {
        for x in 0..w {
            let g = field.gradient(x, y, cell_size_m);
            let n = crate::linalg::Vec3::new(-g.x, -g.y, 1.0).normalized();
            let _ = writeln!(text, "vn {} {} {}", n.x, n.y, n.z);
        }
    }
    for y in 0..h.saturating_sub(1) {
        for x in 0..w.saturating_sub(1) {
            let i0 = y * w + x + 1;
            let i1 = i0 + 1;
            let i2 = (y + 1) * w + x + 1;
            let i3 = i2 + 1;
            let _ = writeln!(text, "f {i0}//{i0} {i1}//{i1} {i3}//{i3}");
            let _ = writeln!(text, "f {i0}//{i0} {i3}//{i3} {i2}//{i2}");
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
    fn obj_counts_match_the_grid() {
        let b = Boundary::new(BoundaryKind::Rectangular, 5, 4);
        let f = ScalarField::filled(b, 3.0);
        let path = std::env::temp_dir().join("geogenesis_obj_test.obj");
        write(&path, &f, 10.0).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().filter(|l| l.starts_with("v ")).count(), 20);
        assert_eq!(text.lines().filter(|l| l.starts_with("vn ")).count(), 20);
        // (w-1)*(h-1)*2 triangles.
        assert_eq!(text.lines().filter(|l| l.starts_with("f ")).count(), 4 * 3 * 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn flat_mesh_normals_point_up() {
        let b = Boundary::new(BoundaryKind::Rectangular, 4, 4);
        let f = ScalarField::filled(b, 3.0);
        let path = std::env::temp_dir().join("geogenesis_obj_flat.obj");
        write(&path, &f, 10.0).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let first = text.lines().find(|l| l.starts_with("vn ")).unwrap();
        assert!(first.ends_with("1"), "flat normal should be +z: {first}");
        std::fs::remove_file(&path).ok();
    }
}