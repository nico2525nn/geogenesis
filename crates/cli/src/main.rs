use clap::{Parser, Subcommand};
use geogenesis_core::export::{ExportFormat, Exporter};
use geogenesis_core::sim::{Simulation, SimulationConfig};
use geogenesis_core::world::WorldConfig;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "geogenesis", about = "GeoGenesis terrain generation engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 地形を生成
    Generate {
        /// グリッド幅
        #[arg(short, long, default_value = "256")]
        width: u32,

        /// グリッド高さ
        #[arg(short, long, default_value = "256")]
        height: u32,

        /// シード値
        #[arg(short, long, default_value = "42")]
        seed: u64,

        /// 出力パス
        #[arg(short, long, default_value = "output.obj")]
        output: PathBuf,

        /// 出力形式 (obj, xyz, raw)
        #[arg(short, long, default_value = "obj")]
        format: String,
    },

    /// 統計情報を表示
    Info {
        /// 入力ファイル（標高データ）
        #[arg(short, long)]
        input: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Generate { width, height, seed, output, format } => {
            let world = WorldConfig {
                world_name: "Generated".into(),
                seed,
                grid_size: (width, height),
                ..WorldConfig::default()
            };

            let config = SimulationConfig::default();
            let mut sim = Simulation::new(world, config);
            sim.generate_initial_heightmap();
            sim.run();

            let fmt = match format.as_str() {
                "obj" => ExportFormat::Obj,
                "xyz" => ExportFormat::XyzAscii,
                "raw" => ExportFormat::RawF64,
                _ => {
                    eprintln!("Unknown format: {}. Use obj, xyz, or raw.", format);
                    std::process::exit(1);
                }
            };

            match Exporter::export(&sim.heightmap, fmt, output.to_str().unwrap()) {
                Ok(_) => println!("Terrain exported to {}", output.display()),
                Err(e) => eprintln!("Export error: {}", e),
            }
        }

        Commands::Info { input: _ } => {
            println!("GeoGenesis CLI");
            println!("Use `geogenesis generate` to create terrain.");
        }
    }
}
