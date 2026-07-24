# GeoGenesis — Algorithm Reference

## Noise Generation

### Perlin Noise
Gradient noise using a hash-based permutation table. Each grid point has a pseudo-random gradient vector; values between grid points are interpolated using a smoothstep (fade) function.

```
fade(t) = 6t⁵ - 15t⁴ + 10t³
```

### Simplex Noise
Skewed simplex grid noise with lower computational complexity than Perlin.
Uses a 2D simplex (triangle) grid instead of a square grid, reducing the number of contributing corners from 4 to 3.

### Fractal Brownian Motion (fBM)
Multiple octaves of noise layered at decreasing amplitude and increasing frequency:

```
fbm(x) = Σᵢ (amplitudeᵢ × noise(frequencyᵢ × x))
amplitudeᵢ₊₁ = amplitudeᵢ × persistence
frequencyᵢ₊₁ = frequencyᵢ × lacunarity
```

## Hydrological Erosion

### Stream Power Model
```
E = Kf · A^m · S^n · dt
```
Where:
- `E` = erosion depth
- `Kf` = erodibility coefficient
- `A` = upstream contributing area
- `S` = local slope
- `m, n` = empirical exponents (typically m=0.5, n=0.5–1.0)

### D8 Flow Direction
Single-flow-direction algorithm: each cell drains into the steepest of its 8 neighbors.

### MFD (Multiple Flow Direction)
Flow is partitioned among all lower neighbors weighted by slope:

```
weightⱼ = (slopeⱼ)^p / Σ(slopeₖ)^p
```
Where `p ≈ 1.1` controls dispersion.

## Tectonic Simulation

### Voronoi Plate Generation
Plates are distributed via Voronoi tessellation with centroid perturbation. Each plate receives a random velocity vector; relative motion at boundaries determines stress distribution.

### Uplift Map
Uplift rate per cell = base uplift × (noise value + boundary proximity factor).

## Hillslope Processes

### Soil Creep (Linear Diffusion)
```
∂h/∂t = D · ∇²h
```

### Landslide Threshold
When slope exceeds the critical angle, excess material is displaced downslope.

## Climate

### Biome Classification
Extended Whittaker diagram: temperature and precipitation determine biome type, with elevation and latitude modifiers.

| Temperature | Precipitation | Biome |
|------------|---------------|-------|
| > 20°C | > 2000mm | Tropical Rainforest |
| > 18°C | < 500mm | Desert |
| 10-18°C | > 800mm | Temperate Forest |
| 5-10°C | > 600mm | Coniferous Forest |
| < -10°C | any | Ice Field |

## References

- Whipple & Tucker (1999) — Stream Power Model
- Braun & Willett (2013) — FastScape
- Tucker et al. (2001) — CHILD
- Salles et al. (2018) — Badlands
- Shobe et al. (2017) — SPACE Model
