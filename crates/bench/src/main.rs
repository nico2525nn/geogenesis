use geogenesis_core::sim::{Simulation, SimulationConfig};
use geogenesis_core::world::WorldConfig;
use std::time::Instant;

fn main() {
    println!("GeoGenesis Benchmark Suite");
    println!("=========================\n");

    // 512x512 ベンチマーク
    bench_size("512x512", 512, 512);
    // 1024x1024 ベンチマーク
    bench_size("1024x1024", 1024, 1024);
}

fn bench_size(label: &str, width: u32, height: u32) {
    let world = WorldConfig {
        world_name: format!("bench_{}x{}", width, height),
        seed: 42,
        grid_size: (width, height),
        ..WorldConfig::default()
    };

    let config = SimulationConfig::default();

    // 初期化時間
    let start = Instant::now();
    let mut sim = Simulation::new(world, config);
    let init_time = start.elapsed();

    // 標高生成時間
    let start = Instant::now();
    sim.generate_initial_heightmap();
    let gen_time = start.elapsed();

    // シミュレーション時間（10ステップ）
    let start = Instant::now();
    for _ in 0..10 {
        sim.step(1000.0);
    }
    let step_time = start.elapsed();

    println!("{}:", label);
    println!("  Initialization:  {:8.2?}", init_time);
    println!("  Heightmap gen:   {:8.2?}", gen_time);
    println!("  10 steps (1ky):  {:8.2?}", step_time);
    println!("  Memory:          {} cells", sim.heightmap.data.len());
    println!("  Elevation range: {:.1} - {:.1} m",
        sim.heightmap.min(), sim.heightmap.max());
    println!();
}
