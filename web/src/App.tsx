import React, { useState, useCallback, useRef, useEffect } from 'react';
import { createEngine, EngineHandle } from './wasm/engine';
import TerrainViewer from './components/TerrainViewer';
import ParameterPanel from './components/ParameterPanel';
import TerrainAnalysis from './components/TerrainAnalysis';
import type { TerrainStatistics } from './types';

const App: React.FC = () => {
  const [engine, setEngine] = useState<EngineHandle | null>(null);
  const [heightmap, setHeightmap] = useState<Float64Array | null>(null);
  const [gridSize, setGridSize] = useState<[number, number]>([256, 256]);
  const [stats, setStats] = useState<TerrainStatistics | null>(null);
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState('Initializing...');

  const engineRef = useRef<EngineHandle | null>(null);

  useEffect(() => {
    (async () => {
      const eng = await createEngine();
      engineRef.current = eng;
      setEngine(eng);
      setStatus('Ready — generate terrain to begin');
    })();
  }, []);

  const generateTerrain = useCallback(async (params?: Record<string, number>) => {
    if (!engineRef.current) return;
    setLoading(true);
    setStatus('Generating terrain...');

    try {
      const worldJson = JSON.stringify({
        worldName: 'Generated',
        seed: params?.seed ?? 42,
        gridSize: [params?.width ?? 256, params?.height ?? 256],
        spatialScaleKm: params?.spatialScale ?? 100,
        verticalScaleM: params?.verticalScale ?? 5000,
        seaLevelM: 0,
        planetRadiusM: 6371000,
        coordinateSystem: 'planar',
        boundary: 'rectangular',
      });

      const configJson = JSON.stringify({
        totalTimeYears: params?.totalTime ?? 1_000_000,
        baseTimeStepYears: 1000,
        adaptiveTimestep: false,
        cflCondition: false,
        maxIterations: 100000,
        noise: {
          noiseType: params?.noiseType ?? 'perlin',
          octaves: params?.octaves ?? 6,
          lacunarity: params?.lacunarity ?? 2.0,
          persistence: params?.persistence ?? 0.5,
          anisotropy: params?.anisotropy ?? 0,
        },
        tectonic: {
          plateCount: params?.plateCount ?? 8,
          avgUpliftRateMmYr: params?.upliftRate ?? 1.0,
          hotspotCount: params?.hotspotCount ?? 5,
        },
        fluvial: {
          kf: params?.kf ?? 1e-5,
          kd: 0.01,
          channelSlopeIndex: params?.channelSlope ?? 0.5,
        },
        hillslope: {
          diffusionCoef: 1e-4,
          criticalSlopeDeg: 35,
          soilProductionRate: 1e-4,
        },
      });

      await engineRef.current.init(worldJson, configJson);
      await engineRef.current.generateInitial();

      if ((params?.erosionSteps ?? 0) > 0) {
        setStatus('Running erosion simulation...');
        for (let i = 0; i < (params?.erosionSteps ?? 10); i++) {
          await engineRef.current.step(1000);
        }
      }

      const data = await engineRef.current.getHeightmap();
      const w = await engineRef.current.getWidth();
      const h = await engineRef.current.getHeight();

      setHeightmap(data);
      setGridSize([w, h]);

      const statsJson = await engineRef.current.getStatistics();
      setStats(JSON.parse(statsJson));
      setStatus(`Done — ${w}×${h} (${data.length.toLocaleString()} cells)`);
    } catch (e) {
      setStatus(`Error: ${e}`);
    } finally {
      setLoading(false);
    }
  }, []);

  return (
    <div style={{ display: 'flex', width: '100vw', height: '100vh' }}>
      {/* 3D Viewport */}
      <div style={{ flex: 1, position: 'relative' }}>
        <TerrainViewer
          heightmap={heightmap}
          width={gridSize[0]}
          height={gridSize[1]}
        />
        {/* Status overlay */}
        <div style={{
          position: 'absolute', bottom: 16, left: 16,
          background: 'rgba(0,0,0,0.7)', padding: '8px 16px',
          borderRadius: 8, fontSize: 13, color: '#aaa',
        }}>
          {status}
        </div>
      </div>

      {/* Right panel */}
      <div style={{
        width: 380, background: '#111116', borderLeft: '1px solid #2a2a35',
        display: 'flex', flexDirection: 'column', overflow: 'hidden',
      }}>
        <ParameterPanel onGenerate={generateTerrain} loading={loading} />
        {stats && <TerrainAnalysis stats={stats} />}
      </div>
    </div>
  );
};

export default App;
