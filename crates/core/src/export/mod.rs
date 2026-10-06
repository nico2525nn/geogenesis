//! Export writers: raw f64, XYZ text, OBJ mesh, 16-bit PNG heightmap.
//!
//! Dependency-free by design: the PNG writer implements stored-block DEFLATE
//! and CRC32 by hand (about 120 lines) so the core stays dependency-free and
//! the WASM binary stays small.

pub mod obj;
pub mod png;
pub mod raw;
pub mod xyz;

/// Export format selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    RawF64,
    XyzAscii,
    Obj,
    Png16,
}

/// Writes `data` in `format` to `path`.
pub fn write_export(
    path: &std::path::Path,
    format: ExportFormat,
    field: &crate::grid::ScalarField,
    cell_size_m: f64,
) -> Result<(), crate::GeoError> {
    match format {
        ExportFormat::RawF64 => raw::write(path, field),
        ExportFormat::XyzAscii => xyz::write(path, field, cell_size_m),
        ExportFormat::Obj => obj::write(path, field, cell_size_m),
        ExportFormat::Png16 => png::write_heightmap(path, field),
    }
}

/// Parses a format name from the CLI and UI.
pub fn format_named(name: &str) -> Option<ExportFormat> {
    match name {
        "raw" | "f64" | "bin" => Some(ExportFormat::RawF64),
        "xyz" | "asc" | "txt" => Some(ExportFormat::XyzAscii),
        "obj" => Some(ExportFormat::Obj),
        "png" | "png16" => Some(ExportFormat::Png16),
        _ => None,
    }
}
