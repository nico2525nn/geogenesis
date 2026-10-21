//! Dependency-free NetCDF-3 classic (CDF-1) writer for gridded engine fields.
//!
//! The writer emits the classic file format specified in the
//! [NetCDF format specification](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html):
//!
//! ```text
//! header: magic "CDF\x01" | numrecs | dim_list | gatt_list | var_list
//! data:   per-variable values, big-endian, row-major (last dimension fastest)
//! ```
//!
//! Every field becomes a two-dimensional `NC_FLOAT` variable (`NC_DOUBLE` for
//! the `_f64` functions) over the shared dimensions `y` (slowest, first) and
//! `x` (fastest, second), so `values[y * width + x]`. The header also carries a
//! global `title` attribute (`GeoGenesis`, overridable); an optional `units`
//! attribute can be attached per field via [`NetCdfField`], and additional
//! global text attributes (for example `sea_level_m`) can be supplied with
//! [`write_netcdf_fields_with_attrs`].
//!
//! There is no record (unlimited) dimension, so `numrecs` is written as `0`,
//! which is what the reference library writes for recordless files. (The value
//! `0xFFFFFFFF` is the format's `STREAMING` marker for an indeterminate record
//! count and is not valid for a fixed-schema file like this one.) Variables are
//! written in alphabetical order, as the classic format expects, each starting
//! at a 32-bit `begin` offset. Classic offsets are signed 32-bit; files that
//! would exceed that limit are rejected with a clear error rather than silently
//! wrapping (the 64-bit offset format would be needed for larger grids).
//!
//! Files are deterministic and accepted by netCDF-C (`nc_open`, `ncdump`),
//! SciPy's reader (`scipy.io.netcdf_file`), xarray and other NetCDF-3 readers.
//! Output is streamed: the header is built in memory (a few hundred bytes plus
//! the variable names) and each row is encoded into one small reusable buffer,
//! so memory use is independent of the number of rows.

use crate::error::{GeoError, GeoResult};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// File magic: `CDF` plus version 1 (classic format).
const MAGIC: [u8; 4] = *b"CDF\x01";

/// Tag introducing the dimension list.
const NC_DIMENSION: u32 = 10;

/// Tag introducing the variable list.
const NC_VARIABLE: u32 = 11;

/// Tag introducing an attribute list.
const NC_ATTRIBUTE: u32 = 12;

/// External type: `char` (also used for string attribute values).
const NC_CHAR: u32 = 2;

/// External type: 32-bit IEEE float.
const NC_FLOAT: u32 = 5;

/// External type: 64-bit IEEE float.
const NC_DOUBLE: u32 = 6;

/// Dimension id of the `y` dimension (the slowest-varying index).
const DIM_Y: u32 = 0;

/// Dimension id of the `x` dimension (the fastest-varying index).
const DIM_X: u32 = 1;

/// Tag/count pair of an absent list.
const ABSENT: u32 = 0;

/// Global `title` attribute value.
const TITLE: &str = "GeoGenesis";

/// Classic `begin` offsets are `NON_NEG` (non-negative signed 32-bit integers).
const MAX_OFFSET: u64 = i32::MAX as u64;

/// External data type of a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleType {
    /// `NC_FLOAT` (5): 4 bytes per value.
    Float,
    /// `NC_DOUBLE` (6): 8 bytes per value.
    Double,
}

impl SampleType {
    /// NetCDF external type tag.
    fn tag(self) -> u32 {
        match self {
            Self::Float => NC_FLOAT,
            Self::Double => NC_DOUBLE,
        }
    }

    /// External size of one value in bytes.
    fn bytes(self) -> u64 {
        match self {
            Self::Float => 4,
            Self::Double => 8,
        }
    }
}

/// Borrowed view of one field's samples.
#[derive(Clone, Copy)]
enum Values<'a> {
    /// Single-precision samples.
    F32(&'a [f32]),
    /// Double-precision samples.
    F64(&'a [f64]),
}

impl Values<'_> {
    /// Number of samples.
    fn len(self) -> usize {
        match self {
            Self::F32(values) => values.len(),
            Self::F64(values) => values.len(),
        }
    }

    /// External type used for this slice.
    fn sample_type(self) -> SampleType {
        match self {
            Self::F32(_) => SampleType::Float,
            Self::F64(_) => SampleType::Double,
        }
    }
}

/// Internal description of one variable.
struct Field<'a> {
    /// NetCDF variable name.
    name: &'a str,
    /// Optional `units` attribute.
    units: Option<&'a str>,
    /// Row-major samples (`y` slowest).
    values: Values<'a>,
}

/// One float field to store with [`write_netcdf_fields`] or
/// [`encode_netcdf_fields`].
///
/// `values` is row-major with `y` as the slowest dimension:
/// `values[y * width + x]`.
#[derive(Clone, Copy, Debug)]
pub struct NetCdfField<'a> {
    /// Variable name; must be a valid netCDF name (`[A-Za-z_][A-Za-z0-9_.@+-]*`).
    pub name: &'a str,
    /// Optional `units` string attribute (`None` omits the attribute).
    pub units: Option<&'a str>,
    /// `width * height` samples in row-major order.
    pub values: &'a [f32],
}

/// Writes float fields as a NetCDF-3 classic file.
///
/// Every field becomes an `NC_FLOAT` variable on the shared `y` (`height`,
/// slowest) and `x` (`width`, fastest) dimensions.
///
/// # Errors
///
/// Returns [`GeoError::InvalidGrid`] for zero or oversized dimensions,
/// [`GeoError::InvalidConfig`] for invalid or duplicate variable names,
/// mismatched sample counts, or files whose classic 32-bit offsets would
/// exceed the format limit, and [`GeoError::Io`] when the file cannot be
/// written.
pub fn write_netcdf(
    path: &Path,
    width: u32,
    height: u32,
    fields: &[(&str, &[f32])],
) -> GeoResult<()> {
    let fields: Vec<Field<'_>> = fields
        .iter()
        .map(|&(name, values)| Field {
            name,
            units: None,
            values: Values::F32(values),
        })
        .collect();
    write_impl(path, width, height, &[], &fields)
}

/// Writes float fields with optional per-field `units` attributes.
///
/// # Errors
///
/// Same as [`write_netcdf`].
pub fn write_netcdf_fields(
    path: &Path,
    width: u32,
    height: u32,
    fields: &[NetCdfField<'_>],
) -> GeoResult<()> {
    let fields: Vec<Field<'_>> = fields
        .iter()
        .map(|field| Field {
            name: field.name,
            units: field.units,
            values: Values::F32(field.values),
        })
        .collect();
    write_impl(path, width, height, &[], &fields)
}

/// Writes float fields with optional per-field `units` and extra global text
/// attributes.
///
/// `global_attrs` are `(name, text)` pairs written as `NC_CHAR` global
/// attributes after the `title` attribute; a caller-supplied `title` overrides
/// the default `GeoGenesis`. They carry metadata such as `sea_level_m`.
///
/// # Errors
///
/// Same as [`write_netcdf`], plus [`GeoError::InvalidConfig`] for invalid,
/// duplicate or NUL-containing global attributes.
pub fn write_netcdf_fields_with_attrs(
    path: &Path,
    width: u32,
    height: u32,
    global_attrs: &[(&str, &str)],
    fields: &[NetCdfField<'_>],
) -> GeoResult<()> {
    let fields: Vec<Field<'_>> = fields
        .iter()
        .map(|field| Field {
            name: field.name,
            units: field.units,
            values: Values::F32(field.values),
        })
        .collect();
    write_impl(path, width, height, global_attrs, &fields)
}

/// Encodes float fields as a NetCDF-3 classic file in memory.
///
/// The bytes are deterministic. This is convenient for tests, WASM callers and
/// callers that handle file writing themselves; prefer [`write_netcdf`] for
/// large grids, because it streams rows to the file.
///
/// # Errors
///
/// Same as [`write_netcdf`].
pub fn encode_netcdf(width: u32, height: u32, fields: &[(&str, &[f32])]) -> GeoResult<Vec<u8>> {
    let fields: Vec<Field<'_>> = fields
        .iter()
        .map(|&(name, values)| Field {
            name,
            units: None,
            values: Values::F32(values),
        })
        .collect();
    encode_impl(width, height, &[], &fields)
}

/// Encodes float fields with optional per-field `units` attributes in memory.
///
/// # Errors
///
/// Same as [`write_netcdf`].
pub fn encode_netcdf_fields(
    width: u32,
    height: u32,
    fields: &[NetCdfField<'_>],
) -> GeoResult<Vec<u8>> {
    let fields: Vec<Field<'_>> = fields
        .iter()
        .map(|field| Field {
            name: field.name,
            units: field.units,
            values: Values::F32(field.values),
        })
        .collect();
    encode_impl(width, height, &[], &fields)
}

/// Encodes float fields with optional per-field `units` and additional global
/// text attributes in memory.
///
/// # Errors
///
/// Same as [`write_netcdf_fields_with_attrs`].
pub fn encode_netcdf_fields_with_attrs(
    width: u32,
    height: u32,
    global_attrs: &[(&str, &str)],
    fields: &[NetCdfField<'_>],
) -> GeoResult<Vec<u8>> {
    let fields: Vec<Field<'_>> = fields
        .iter()
        .map(|field| Field {
            name: field.name,
            units: field.units,
            values: Values::F32(field.values),
        })
        .collect();
    encode_impl(width, height, global_attrs, &fields)
}

/// Writes double-precision fields as a NetCDF-3 classic file (`NC_DOUBLE`).
///
/// # Errors
///
/// Same as [`write_netcdf`].
pub fn write_netcdf_f64(
    path: &Path,
    width: u32,
    height: u32,
    fields: &[(&str, &[f64])],
) -> GeoResult<()> {
    let fields: Vec<Field<'_>> = fields
        .iter()
        .map(|&(name, values)| Field {
            name,
            units: None,
            values: Values::F64(values),
        })
        .collect();
    write_impl(path, width, height, &[], &fields)
}

/// Encodes double-precision fields as a NetCDF-3 classic file in memory.
///
/// # Errors
///
/// Same as [`write_netcdf`].
pub fn encode_netcdf_f64(width: u32, height: u32, fields: &[(&str, &[f64])]) -> GeoResult<Vec<u8>> {
    let fields: Vec<Field<'_>> = fields
        .iter()
        .map(|&(name, values)| Field {
            name,
            units: None,
            values: Values::F64(values),
        })
        .collect();
    encode_impl(width, height, &[], &fields)
}

/// Writes a prepared field list to a file, creating parent directories.
fn write_impl(
    path: &Path,
    width: u32,
    height: u32,
    global_attrs: &[(&str, &str)],
    fields: &[Field<'_>],
) -> GeoResult<()> {
    let order = layout(width, height, fields)?;
    let header = header_bytes(width, height, global_attrs, fields, &order)?;
    let mut file = create_output(path)?;
    write_stream(
        &mut file,
        width,
        height,
        fields,
        &order,
        &header,
        &path.display().to_string(),
    )?;
    file.flush()
        .map_err(|e| GeoError::Io(format!("{}: {e}", path.display())))
}

/// Encodes a prepared field list into a byte vector.
fn encode_impl(
    width: u32,
    height: u32,
    global_attrs: &[(&str, &str)],
    fields: &[Field<'_>],
) -> GeoResult<Vec<u8>> {
    let order = layout(width, height, fields)?;
    let header = header_bytes(width, height, global_attrs, fields, &order)?;
    let mut data_bytes = 0u64;
    for &index in &order {
        data_bytes += padded_size(data_bytes_required(
            width,
            height,
            fields[index].values.sample_type(),
        )?);
    }
    let total = header.len() as u64 + data_bytes;
    let capacity = usize::try_from(total).map_err(|_| {
        GeoError::InvalidConfig(
            "NetCDF file does not fit this platform's address space".to_string(),
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    write_stream(
        &mut bytes,
        width,
        height,
        fields,
        &order,
        &header,
        "NetCDF encode",
    )?;
    Ok(bytes)
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

/// Validates dimensions, names and sample counts, then returns the variables in
/// the alphabetical order required by the classic format.
fn layout(width: u32, height: u32, fields: &[Field<'_>]) -> GeoResult<Vec<usize>> {
    if width == 0 || height == 0 {
        return Err(GeoError::InvalidGrid(format!(
            "NetCDF grid must be at least 1x1 (got {width}x{height})"
        )));
    }
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(GeoError::InvalidGrid(format!(
            "NetCDF dimension lengths must fit the classic 32-bit format (got {width}x{height})"
        )));
    }
    let pixels = u64::from(width) * u64::from(height);
    for field in fields {
        validate_netcdf_name(field.name, "variable")?;
        if field.values.len() as u64 != pixels {
            return Err(GeoError::InvalidConfig(format!(
                "NetCDF field `{}` has {} values, expected {pixels} ({width}x{height})",
                field.name,
                field.values.len()
            )));
        }
        if let Some(units) = field.units {
            if units.bytes().any(|byte| byte == 0) {
                return Err(GeoError::InvalidConfig(format!(
                    "NetCDF units for field `{}` contain a NUL byte",
                    field.name
                )));
            }
        }
    }

    let mut order: Vec<usize> = (0..fields.len()).collect();
    order.sort_by(|&a, &b| fields[a].name.cmp(fields[b].name));
    for pair in order.windows(2) {
        if fields[pair[0]].name == fields[pair[1]].name {
            return Err(GeoError::InvalidConfig(format!(
                "duplicate NetCDF variable `{}`",
                fields[pair[0]].name
            )));
        }
    }
    Ok(order)
}

/// Accepts the documented classic ASCII subset of valid netCDF names.
fn validate_netcdf_name(name: &str, kind: &str) -> GeoResult<()> {
    let bytes = name.as_bytes();
    let Some((&first, rest)) = bytes.split_first() else {
        return Err(GeoError::InvalidConfig(format!(
            "NetCDF {kind} name is empty"
        )));
    };
    if !(first.is_ascii_alphanumeric() || first == b'_') {
        return Err(GeoError::InvalidConfig(format!(
            "NetCDF {kind} name `{name}` must start with a letter, digit or underscore"
        )));
    }
    for &byte in rest {
        let allowed =
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'@' | b'+' | b'-');
        if !allowed {
            return Err(GeoError::InvalidConfig(format!(
                "NetCDF {kind} name `{name}` contains an unsupported character"
            )));
        }
    }
    Ok(())
}

/// Validated global text attributes.
struct GlobalAttributes<'a> {
    /// Caller-supplied `title`; `None` means the default [`TITLE`] is used.
    title: Option<&'a str>,
    /// The remaining attributes, in caller order.
    extras: Vec<(&'a str, &'a str)>,
}

/// Validates global text attributes and splits them into title and extras.
fn prepare_global_attributes<'a>(
    global_attrs: &'a [(&'a str, &'a str)],
) -> GeoResult<GlobalAttributes<'a>> {
    let mut title = None;
    let mut extras = Vec::with_capacity(global_attrs.len());
    for (index, &(name, value)) in global_attrs.iter().enumerate() {
        validate_netcdf_name(name, "global attribute")?;
        if value.bytes().any(|byte| byte == 0) {
            return Err(GeoError::InvalidConfig(format!(
                "NetCDF global attribute `{name}` contains a NUL byte"
            )));
        }
        if global_attrs[index + 1..]
            .iter()
            .any(|(other, _)| *other == name)
        {
            return Err(GeoError::InvalidConfig(format!(
                "duplicate NetCDF global attribute `{name}`"
            )));
        }
        if name == "title" {
            title = Some(value);
        } else {
            extras.push((name, value));
        }
    }
    Ok(GlobalAttributes { title, extras })
}

/// Bytes of one variable's data (without the 4-byte padding).
fn data_bytes_required(width: u32, height: u32, sample: SampleType) -> GeoResult<u64> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(sample.bytes()))
        .ok_or_else(|| {
            GeoError::InvalidConfig(
                "NetCDF variable is too large to address in this process".to_string(),
            )
        })
}

/// Rounds a byte count up to the next multiple of four.
fn padded_size(bytes: u64) -> u64 {
    (bytes + 3) & !3
}

/// Builds the complete classic header, patching each variable's `begin` offset
/// once the header length is known.
fn header_bytes(
    width: u32,
    height: u32,
    global_attrs: &[(&str, &str)],
    fields: &[Field<'_>],
    order: &[usize],
) -> GeoResult<Vec<u8>> {
    let GlobalAttributes { title, extras } = prepare_global_attributes(global_attrs)?;
    let mut out = Vec::with_capacity(128 + fields.len() * 96 + global_attrs.len() * 32);
    out.extend_from_slice(&MAGIC);
    put_u32(&mut out, 0); // numrecs: no record dimension

    // Dimensions: y is the slowest (row), x the fastest (column).
    put_u32(&mut out, NC_DIMENSION);
    put_u32(&mut out, 2);
    put_name(&mut out, "y");
    put_u32(&mut out, height);
    put_name(&mut out, "x");
    put_u32(&mut out, width);

    // Global attributes: `title` first, then any caller-supplied extras.
    put_u32(&mut out, NC_ATTRIBUTE);
    put_u32(&mut out, 1 + extras.len() as u32);
    write_text_attribute(&mut out, "title", title.unwrap_or(TITLE));
    for &(name, value) in &extras {
        write_text_attribute(&mut out, name, value);
    }

    // Variable list.
    if fields.is_empty() {
        put_u32(&mut out, ABSENT);
        put_u32(&mut out, 0);
        return Ok(out);
    }
    put_u32(&mut out, NC_VARIABLE);
    put_u32(&mut out, fields.len() as u32);

    let mut begin_positions = Vec::with_capacity(order.len());
    for &index in order {
        let field = &fields[index];
        let sample = field.values.sample_type();
        put_name(&mut out, field.name);
        put_u32(&mut out, 2); // rank
        put_u32(&mut out, DIM_Y);
        put_u32(&mut out, DIM_X);
        match field.units {
            Some(units) => {
                put_u32(&mut out, NC_ATTRIBUTE);
                put_u32(&mut out, 1);
                write_text_attribute(&mut out, "units", units);
            }
            None => {
                put_u32(&mut out, ABSENT);
                put_u32(&mut out, 0);
            }
        }
        put_u32(&mut out, sample.tag());
        let vsize = padded_size(data_bytes_required(width, height, sample)?);
        let vsize = u32::try_from(vsize).map_err(|_| {
            GeoError::InvalidConfig(format!(
                "NetCDF variable `{}` is too large for the classic format",
                field.name
            ))
        })?;
        put_u32(&mut out, vsize);
        begin_positions.push(out.len());
        put_u32(&mut out, 0); // begin placeholder, patched below
    }

    // Absolute offsets: the header is fixed width, so its length is known now.
    let mut data_offset = out.len() as u64;
    for (&index, &position) in order.iter().zip(&begin_positions) {
        let sample = fields[index].values.sample_type();
        let size = padded_size(data_bytes_required(width, height, sample)?);
        if data_offset > MAX_OFFSET {
            return Err(GeoError::InvalidConfig(format!(
                "NetCDF classic data offset {data_offset} exceeds the 2 GiB format limit; \
                 use a smaller grid or fewer fields"
            )));
        }
        out[position..position + 4].copy_from_slice(&(data_offset as u32).to_be_bytes());
        data_offset += size;
    }
    if data_offset > MAX_OFFSET {
        return Err(GeoError::InvalidConfig(
            "NetCDF classic file would exceed the 2 GiB format limit; use a smaller grid or \
             fewer fields"
                .to_string(),
        ));
    }
    Ok(out)
}

/// Appends a 32-bit big-endian integer.
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

/// Appends one `NC_CHAR` attribute: name, type, length, padded text.
fn write_text_attribute(out: &mut Vec<u8>, name: &str, value: &str) {
    put_name(out, name);
    put_u32(out, NC_CHAR);
    put_u32(out, value.len() as u32);
    put_padded_bytes(out, value.as_bytes());
}

/// Appends a name: 32-bit length followed by null-padded UTF-8 bytes.
fn put_name(out: &mut Vec<u8>, name: &str) {
    put_u32(out, name.len() as u32);
    put_padded_bytes(out, name.as_bytes());
}

/// Appends bytes padded with nulls to a 4-byte boundary.
fn put_padded_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(bytes);
    while out.len() & 3 != 0 {
        out.push(0);
    }
}

/// Streams the header and each variable's big-endian data to `writer`.
fn write_stream<W: Write>(
    writer: &mut W,
    width: u32,
    height: u32,
    fields: &[Field<'_>],
    order: &[usize],
    header: &[u8],
    context: &str,
) -> GeoResult<()> {
    let io = |error: std::io::Error| GeoError::Io(format!("{context}: {error}"));
    writer.write_all(header).map_err(io)?;
    let columns = width as usize;
    let mut row = Vec::with_capacity(columns * 8);
    for &index in order {
        match fields[index].values {
            Values::F32(values) => {
                for y in 0..height as usize {
                    let start = y * columns;
                    row.clear();
                    for &value in &values[start..start + columns] {
                        row.extend_from_slice(&value.to_be_bytes());
                    }
                    writer.write_all(&row).map_err(io)?;
                }
            }
            Values::F64(values) => {
                for y in 0..height as usize {
                    let start = y * columns;
                    row.clear();
                    for &value in &values[start..start + columns] {
                        row.extend_from_slice(&value.to_be_bytes());
                    }
                    writer.write_all(&row).map_err(io)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u32(bytes: &[u8], pos: &mut usize) -> u32 {
        let value = u32::from_be_bytes(bytes[*pos..*pos + 4].try_into().unwrap());
        *pos += 4;
        value
    }

    fn read_name(bytes: &[u8], pos: &mut usize) -> String {
        let len = read_u32(bytes, pos) as usize;
        let name = String::from_utf8(bytes[*pos..*pos + len].to_vec()).expect("UTF-8 name");
        *pos += padded_size(len as u64) as usize;
        name
    }

    fn external_size(nc_type: u32) -> usize {
        match nc_type {
            NC_CHAR | 1 => 1,  // NC_CHAR, NC_BYTE
            3 => 2,            // NC_SHORT
            NC_FLOAT | 4 => 4, // NC_FLOAT, NC_INT
            NC_DOUBLE => 8,
            other => panic!("unsupported netCDF type {other}"),
        }
    }

    /// A parsed attribute; `values` holds only the meaningful bytes.
    struct ParsedAttr {
        name: String,
        nc_type: u32,
        values: Vec<u8>,
    }

    /// A parsed variable.
    struct ParsedVar {
        name: String,
        dimids: Vec<u32>,
        attrs: Vec<ParsedAttr>,
        nc_type: u32,
        vsize: u32,
        begin: usize,
    }

    /// Minimal reader used to verify the encoder against the format grammar.
    struct ParsedNetCdf {
        numrecs: u32,
        dims: Vec<(String, u32)>,
        gatts: Vec<ParsedAttr>,
        vars: Vec<ParsedVar>,
        header_len: usize,
        bytes: Vec<u8>,
    }

    impl ParsedNetCdf {
        fn var(&self, name: &str) -> &ParsedVar {
            self.vars
                .iter()
                .find(|var| var.name == name)
                .unwrap_or_else(|| panic!("missing NetCDF variable `{name}`"))
        }

        fn var_attr<'a>(&'a self, var: &'a ParsedVar, name: &str) -> &'a [u8] {
            &var.attrs
                .iter()
                .find(|attr| attr.name == name)
                .unwrap_or_else(|| panic!("missing attribute `{name}`"))
                .values
        }

        fn dim_len(&self, name: &str) -> u32 {
            self.dims
                .iter()
                .find(|(dim, _)| dim == name)
                .unwrap_or_else(|| panic!("missing dimension `{name}`"))
                .1
        }

        fn sample_count(&self, var: &ParsedVar) -> usize {
            var.dimids
                .iter()
                .map(|&dimid| self.dims[dimid as usize].1 as usize)
                .product()
        }

        fn f32_values(&self, name: &str) -> Vec<f32> {
            let var = self.var(name);
            assert_eq!(var.nc_type, NC_FLOAT);
            let count = self.sample_count(var);
            let mut pos = var.begin;
            (0..count)
                .map(|_| f32::from_bits(read_u32(&self.bytes, &mut pos)))
                .collect()
        }

        fn f64_values(&self, name: &str) -> Vec<f64> {
            let var = self.var(name);
            assert_eq!(var.nc_type, NC_DOUBLE);
            let count = self.sample_count(var);
            let mut pos = var.begin;
            (0..count)
                .map(|_| {
                    let value = f64::from_bits(u64::from_be_bytes(
                        self.bytes[pos..pos + 8].try_into().unwrap(),
                    ));
                    pos += 8;
                    value
                })
                .collect()
        }
    }

    fn read_attrs(bytes: &[u8], pos: &mut usize) -> Vec<ParsedAttr> {
        let tag = read_u32(bytes, pos);
        let count = read_u32(bytes, pos);
        if tag == ABSENT {
            assert_eq!(count, 0, "absent list must have a zero count");
            return Vec::new();
        }
        assert_eq!(tag, NC_ATTRIBUTE, "attribute list tag");
        let mut attrs = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = read_name(bytes, pos);
            let nc_type = read_u32(bytes, pos);
            let nelems = read_u32(bytes, pos) as usize;
            let size = nelems * external_size(nc_type);
            let values = bytes[*pos..*pos + size].to_vec();
            *pos += padded_size(size as u64) as usize;
            attrs.push(ParsedAttr {
                name,
                nc_type,
                values,
            });
        }
        attrs
    }

    fn parse_netcdf(bytes: &[u8]) -> ParsedNetCdf {
        assert!(bytes.len() >= 32);
        assert_eq!(&bytes[..4], &MAGIC, "file magic");
        let mut pos = 4;
        let numrecs = read_u32(bytes, &mut pos);

        let tag = read_u32(bytes, &mut pos);
        let count = read_u32(bytes, &mut pos);
        assert_eq!(tag, NC_DIMENSION, "dimension list tag");
        let mut dims = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = read_name(bytes, &mut pos);
            let len = read_u32(bytes, &mut pos);
            dims.push((name, len));
        }

        let gatts = read_attrs(bytes, &mut pos);

        let tag = read_u32(bytes, &mut pos);
        let count = read_u32(bytes, &mut pos);
        let mut vars = Vec::new();
        if tag != ABSENT {
            assert_eq!(tag, NC_VARIABLE, "variable list tag");
            for _ in 0..count {
                let name = read_name(bytes, &mut pos);
                let rank = read_u32(bytes, &mut pos) as usize;
                let dimids = (0..rank).map(|_| read_u32(bytes, &mut pos)).collect();
                let attrs = read_attrs(bytes, &mut pos);
                let nc_type = read_u32(bytes, &mut pos);
                let vsize = read_u32(bytes, &mut pos);
                let begin = read_u32(bytes, &mut pos) as usize;
                vars.push(ParsedVar {
                    name,
                    dimids,
                    attrs,
                    nc_type,
                    vsize,
                    begin,
                });
            }
        } else {
            assert_eq!(count, 0, "absent list must have a zero count");
        }
        let header_len = pos;

        // The header is always a multiple of four bytes, and every variable
        // starts after it.
        assert_eq!(header_len & 3, 0, "header alignment");
        for var in &vars {
            assert!(var.begin >= header_len, "variable before end of header");
            assert_eq!(var.begin & 3, 0, "variable data alignment");
        }
        // Variables are contiguous in header order.
        for pair in vars.windows(2) {
            assert_eq!(pair[0].begin + pair[0].vsize as usize, pair[1].begin);
        }
        if let Some(last) = vars.last() {
            assert_eq!(last.begin + last.vsize as usize, bytes.len());
        }

        ParsedNetCdf {
            numrecs,
            dims,
            gatts,
            vars,
            header_len,
            bytes: bytes.to_vec(),
        }
    }

    fn temp_path(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("geogenesis_nc_{}_{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join("out.nc")
    }

    fn grid(width: u32, height: u32) -> Vec<f32> {
        (0..(width * height))
            .map(|index| {
                let x = (index % width) as f32;
                let y = (index / width) as f32;
                y * 100.0 + x
            })
            .collect()
    }

    #[test]
    fn single_field_round_trips_with_classic_header() {
        let (width, height) = (3u32, 5u32);
        let values = grid(width, height);
        let parsed =
            parse_netcdf(&encode_netcdf(width, height, &[("elevation", &values)]).unwrap());

        assert_eq!(parsed.numrecs, 0, "no record dimension");
        assert_eq!(
            parsed.dims,
            vec![("y".to_string(), 5), ("x".to_string(), 3)]
        );
        assert_eq!(parsed.dim_len("y"), 5);
        assert_eq!(parsed.dim_len("x"), 3);
        assert_eq!(parsed.gatts.len(), 1);
        assert_eq!(parsed.gatts[0].name, "title");
        assert_eq!(parsed.gatts[0].nc_type, NC_CHAR);
        assert_eq!(parsed.gatts[0].values, TITLE.as_bytes());

        assert_eq!(parsed.vars.len(), 1);
        let var = parsed.var("elevation");
        assert_eq!(var.dimids, vec![DIM_Y, DIM_X]);
        assert_eq!(var.nc_type, NC_FLOAT);
        assert_eq!(var.vsize, width * height * 4);
        assert_eq!(var.begin, parsed.header_len);
        assert!(var.attrs.is_empty());
        assert_eq!(var.begin + var.vsize as usize, parsed.bytes.len());

        // Big-endian storage and row-major order with y slowest.
        let first = u32::from_be_bytes([
            parsed.bytes[var.begin],
            parsed.bytes[var.begin + 1],
            parsed.bytes[var.begin + 2],
            parsed.bytes[var.begin + 3],
        ]);
        assert_eq!(first, 0.0f32.to_bits());
        let second = u32::from_be_bytes([
            parsed.bytes[var.begin + 4],
            parsed.bytes[var.begin + 5],
            parsed.bytes[var.begin + 6],
            parsed.bytes[var.begin + 7],
        ]);
        assert_eq!(second, 1.0f32.to_bits());

        let decoded = parsed.f32_values("elevation");
        for (index, expected) in values.iter().enumerate() {
            assert_eq!(
                decoded[index].to_bits(),
                expected.to_bits(),
                "value {index}"
            );
        }
    }

    #[test]
    fn variables_are_sorted_by_name_and_laid_out_contiguously() {
        let (width, height) = (4u32, 2u32);
        let zeta = grid(width, height);
        let alpha: Vec<f32> = zeta.iter().map(|value| value * 2.0).collect();
        let elevation: Vec<f32> = zeta.iter().map(|value| value - 7.0).collect();
        let parsed = parse_netcdf(
            &encode_netcdf(
                width,
                height,
                &[
                    ("zeta", &zeta),
                    ("alpha", &alpha),
                    ("elevation", &elevation),
                ],
            )
            .unwrap(),
        );
        let names: Vec<&str> = parsed.vars.iter().map(|var| var.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "elevation", "zeta"]);
        let mut expected_begin = parsed.header_len;
        for var in &parsed.vars {
            assert_eq!(var.begin, expected_begin);
            assert_eq!(var.vsize, width * height * 4);
            expected_begin += var.vsize as usize;
        }
        assert_eq!(expected_begin, parsed.bytes.len());

        for (name, expected) in [
            ("alpha", &alpha),
            ("elevation", &elevation),
            ("zeta", &zeta),
        ] {
            let decoded = parsed.f32_values(name);
            for (index, value) in expected.iter().enumerate() {
                assert_eq!(decoded[index].to_bits(), value.to_bits(), "{name}[{index}]");
            }
        }
    }

    #[test]
    fn units_attribute_is_written_only_when_requested() {
        let values = grid(2, 3);
        let fields = [
            NetCdfField {
                name: "temperature",
                units: Some("degC"),
                values: &values,
            },
            NetCdfField {
                name: "elevation",
                units: None,
                values: &values,
            },
        ];
        let parsed = parse_netcdf(&encode_netcdf_fields(2, 3, &fields).unwrap());
        let temperature = parsed.var("temperature");
        assert_eq!(temperature.attrs.len(), 1);
        assert_eq!(temperature.attrs[0].name, "units");
        assert_eq!(temperature.attrs[0].nc_type, NC_CHAR);
        assert_eq!(parsed.var_attr(temperature, "units"), b"degC");
        let elevation = parsed.var("elevation");
        assert!(elevation.attrs.is_empty());
    }

    #[test]
    fn custom_global_attributes_are_written_in_order() {
        let values = grid(2, 2);
        let fields = [NetCdfField {
            name: "elevation_m",
            units: Some("m"),
            values: &values,
        }];
        let parsed = parse_netcdf(
            &encode_netcdf_fields_with_attrs(
                2,
                2,
                &[
                    ("sea_level_m", "12.500"),
                    ("title", "GeoGenesis world export"),
                    ("source", "GeoGenesis"),
                ],
                &fields,
            )
            .unwrap(),
        );
        assert_eq!(parsed.gatts.len(), 3);
        assert_eq!(parsed.gatts[0].name, "title");
        assert_eq!(parsed.gatts[0].nc_type, NC_CHAR);
        assert_eq!(parsed.gatts[0].values, b"GeoGenesis world export");
        assert_eq!(parsed.gatts[1].name, "sea_level_m");
        assert_eq!(parsed.gatts[1].values, b"12.500");
        assert_eq!(parsed.gatts[2].name, "source");
        assert_eq!(parsed.gatts[2].values, b"GeoGenesis");
        assert_eq!(parsed.f32_values("elevation_m"), values);

        // Without a caller title the default is still written first.
        let parsed = parse_netcdf(
            &encode_netcdf_fields_with_attrs(2, 2, &[("sea_level_m", "-30.0")], &fields).unwrap(),
        );
        assert_eq!(parsed.gatts.len(), 2);
        assert_eq!(parsed.gatts[0].name, "title");
        assert_eq!(parsed.gatts[0].values, TITLE.as_bytes());
        assert_eq!(parsed.gatts[1].values, b"-30.0");
    }

    #[test]
    fn rejects_invalid_global_attributes() {
        let values = grid(2, 2);
        let fields = [NetCdfField {
            name: "a",
            units: None,
            values: &values,
        }];
        for attrs in [
            vec![("sea level", "1")],
            vec![("bad\0name", "1")],
            vec![("ok", "bad\0value")],
            vec![("dupe", "1"), ("dupe", "2")],
        ] {
            let error = encode_netcdf_fields_with_attrs(2, 2, &attrs, &fields).unwrap_err();
            assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");
        }
    }

    #[test]
    fn double_precision_fields_round_trip() {
        let values: Vec<f64> = vec![
            0.1,
            -2.5,
            f64::from_bits(0x7ff8_0000_0000_1234), // NaN payload
            f64::INFINITY,
            f64::NEG_INFINITY,
            -0.0,
            f64::MIN_POSITIVE,
        ];
        let parsed = parse_netcdf(&encode_netcdf_f64(7, 1, &[("depth", &values)]).unwrap());
        let var = parsed.var("depth");
        assert_eq!(var.nc_type, NC_DOUBLE);
        assert_eq!(var.vsize, 7 * 8);
        assert_eq!(var.begin, parsed.header_len);
        let decoded = parsed.f64_values("depth");
        for (index, expected) in values.iter().enumerate() {
            assert_eq!(
                decoded[index].to_bits(),
                expected.to_bits(),
                "value {index}"
            );
        }
    }

    #[test]
    fn float_bit_patterns_round_trip_exactly() {
        let values: Vec<f32> = vec![
            f32::from_bits(0x7fc1_2345), // NaN with payload
            f32::INFINITY,
            f32::NEG_INFINITY,
            -0.0,
            f32::from_bits(0x0000_0001), // smallest subnormal
            f32::from_bits(0x7f7f_ffff), // largest finite
            1.0e-30,
            1.0e30,
            0.1,
        ];
        let parsed = parse_netcdf(&encode_netcdf(9, 1, &[("values", &values)]).unwrap());
        let decoded = parsed.f32_values("values");
        for (index, expected) in values.iter().enumerate() {
            assert_eq!(
                decoded[index].to_bits(),
                expected.to_bits(),
                "value {index}"
            );
        }
    }

    #[test]
    fn empty_field_list_writes_a_valid_dimension_only_file() {
        let parsed = parse_netcdf(&encode_netcdf(4, 2, &[]).unwrap());
        assert_eq!(parsed.numrecs, 0);
        assert_eq!(
            parsed.dims,
            vec![("y".to_string(), 2), ("x".to_string(), 4)]
        );
        assert!(parsed.vars.is_empty());
        assert_eq!(parsed.gatts.len(), 1);
        assert_eq!(parsed.header_len, parsed.bytes.len());
    }

    #[test]
    fn rejects_invalid_arguments() {
        let empty: [f32; 0] = [];

        let error = encode_netcdf(0, 1, &[("a", &empty)]).unwrap_err();
        assert!(matches!(error, GeoError::InvalidGrid(_)), "{error:?}");

        let error = encode_netcdf(1, 0, &[("a", &empty)]).unwrap_err();
        assert!(matches!(error, GeoError::InvalidGrid(_)), "{error:?}");

        let error = encode_netcdf(u32::MAX, 1, &[("a", &empty)]).unwrap_err();
        assert!(matches!(error, GeoError::InvalidGrid(_)), "{error:?}");

        let values = grid(2, 2);
        let error = encode_netcdf(2, 2, &[("a", &values[..3])]).unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        let error = encode_netcdf(2, 2, &[("", &values)]).unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        for bad in ["bad/name", "has space", "-leading", "bad\0name", "a,b"] {
            let error = encode_netcdf(2, 2, &[(bad, &values)]).unwrap_err();
            assert!(
                matches!(error, GeoError::InvalidConfig(_)),
                "name {bad:?}: {error:?}"
            );
        }

        let error = encode_netcdf(2, 2, &[("a", &values), ("a", &values)]).unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");

        let fields = [NetCdfField {
            name: "a",
            units: Some("bad\0units"),
            values: &values,
        }];
        let error = encode_netcdf_fields(2, 2, &fields).unwrap_err();
        assert!(matches!(error, GeoError::InvalidConfig(_)), "{error:?}");
    }

    #[test]
    fn file_writer_creates_directories_and_matches_the_encoder() {
        let path = temp_path("file");
        let (width, height) = (3u32, 4u32);
        let values = grid(width, height);
        write_netcdf(&path, width, height, &[("elevation", &values)]).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            bytes,
            encode_netcdf(width, height, &[("elevation", &values)]).unwrap()
        );
        let parsed = parse_netcdf(&bytes);
        assert_eq!(parsed.f32_values("elevation"), values);

        let fields_path = temp_path("fields");
        let fields = [NetCdfField {
            name: "elevation",
            units: Some("m"),
            values: &values,
        }];
        write_netcdf_fields(&fields_path, width, height, &fields).unwrap();
        let bytes = std::fs::read(&fields_path).unwrap();
        assert_eq!(bytes, encode_netcdf_fields(width, height, &fields).unwrap());

        for path in [&path, &fields_path] {
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    #[test]
    fn encoding_is_deterministic() {
        let values = grid(5, 3);
        let first = encode_netcdf(5, 3, &[("b", &values), ("a", &values)]).unwrap();
        let second = encode_netcdf(5, 3, &[("a", &values), ("b", &values)]).unwrap();
        // Field order in the call is irrelevant: variables are sorted by name.
        assert_eq!(first, second);
        assert_eq!(
            encode_netcdf(5, 3, &[("b", &values), ("a", &values)]).unwrap(),
            first
        );
    }
}
