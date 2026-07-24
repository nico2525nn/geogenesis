use crate::world::HeightMap;
use std::fs;
use std::io::Write;

/// エクスポート設定
#[derive(Debug, Clone)]
pub enum ExportFormat {
    /// 生バイナリ（f64 配列）
    RawF64,
    /// PNG 標高（16bit グレースケール）
    Png16,
    /// XYZ 点群（テキスト）
    XyzAscii,
    /// OBJ メッシュ
    Obj,
}

/// エクスポート管理
pub struct Exporter;

impl Exporter {
    /// 標高マップを各種形式で出力
    pub fn export(heightmap: &HeightMap, format: ExportFormat, path: &str) -> Result<(), String> {
        match format {
            ExportFormat::RawF64 => Self::export_raw(heightmap, path),
            ExportFormat::XyzAscii => Self::export_xyz(heightmap, path),
            ExportFormat::Obj => Self::export_obj(heightmap, path),
            _ => Err("Format not yet implemented".into()),
        }
    }

    fn export_raw(heightmap: &HeightMap, path: &str) -> Result<(), String> {
        let bytes: Vec<u8> = heightmap.data
            .iter()
            .flat_map(|&v| v.to_le_bytes())
            .collect();
        fs::write(path, &bytes).map_err(|e| e.to_string())
    }

    fn export_xyz(heightmap: &HeightMap, path: &str) -> Result<(), String> {
        let mut f = fs::File::create(path).map_err(|e| e.to_string())?;
        for y in 0..heightmap.height {
            for x in 0..heightmap.width {
                let z = heightmap.data[y * heightmap.width + x];
                writeln!(f, "{} {} {}", x, y, z).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    fn export_obj(heightmap: &HeightMap, path: &str) -> Result<(), String> {
        let w = heightmap.width;
        let h = heightmap.height;
        let mut f = fs::File::create(path).map_err(|e| e.to_string())?;

        // 頂点
        for y in 0..h {
            for x in 0..w {
                let z = heightmap.data[y * w + x];
                writeln!(f, "v {} {} {}", x as f64, y as f64, z).map_err(|e| e.to_string())?;
            }
        }

        // 面
        for y in 0..h - 1 {
            for x in 0..w - 1 {
                let i0 = y * w + x + 1;
                let i1 = i0 + 1;
                let i2 = (y + 1) * w + x + 1;
                let i3 = i2 + 1;
                writeln!(f, "f {} {} {}", i0, i1, i3).map_err(|e| e.to_string())?;
                writeln!(f, "f {} {} {}", i0, i3, i2).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}
