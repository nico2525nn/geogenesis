import React, { useState, useCallback } from 'react';

interface Props {
  onGenerate: (params: Record<string, number>) => void;
  loading: boolean;
}

/** パラメータ定義 */
const PARAM_GROUPS = [
  {
    id: 'world',
    label: 'World',
    params: [
      { id: 'seed', label: 'Seed', min: 0, max: 999999, step: 1, default: 42 },
      { id: 'width', label: 'Width', min: 16, max: 1024, step: 16, default: 256 },
      { id: 'height', label: 'Height', min: 16, max: 1024, step: 16, default: 256 },
      { id: 'spatialScale', label: 'Spatial Scale (km)', min: 1, max: 10000, step: 10, default: 100 },
      { id: 'verticalScale', label: 'Vertical Scale (m)', min: 100, max: 50000, step: 100, default: 5000 },
    ],
  },
  {
    id: 'noise',
    label: 'Noise',
    params: [
      { id: 'noiseType', label: 'Type', options: ['perlin', 'simplex', 'value', 'wavelet'], default: 'perlin' },
      { id: 'octaves', label: 'Octaves', min: 1, max: 16, step: 1, default: 6 },
      { id: 'lacunarity', label: 'Lacunarity', min: 1.0, max: 4.0, step: 0.1, default: 2.0 },
      { id: 'persistence', label: 'Persistence', min: 0.1, max: 1.0, step: 0.05, default: 0.5 },
      { id: 'anisotropy', label: 'Anisotropy', min: 0, max: 1.0, step: 0.05, default: 0 },
    ],
  },
  {
    id: 'tectonic',
    label: 'Tectonic',
    params: [
      { id: 'plateCount', label: 'Plates', min: 1, max: 32, step: 1, default: 8 },
      { id: 'upliftRate', label: 'Uplift Rate (mm/yr)', min: -10, max: 50, step: 0.5, default: 1.0 },
      { id: 'hotspotCount', label: 'Hotspots', min: 0, max: 50, step: 1, default: 5 },
    ],
  },
  {
    id: 'erosion',
    label: 'Erosion',
    params: [
      { id: 'erosionSteps', label: 'Iterations', min: 0, max: 1000, step: 10, default: 10 },
      { id: 'kf', label: 'Kf (erosion)', min: 0.00000001, max: 0.001, step: 0.000001, default: 0.00001 },
      { id: 'channelSlope', label: 'Channel Slope Idx', min: 0.2, max: 0.7, step: 0.05, default: 0.5 },
    ],
  },
];

const ParameterPanel: React.FC<Props> = ({ onGenerate, loading }) => {
  const [params, setParams] = useState<Record<string, number>>(() => {
    const init: Record<string, number> = {};
    for (const group of PARAM_GROUPS) {
      for (const p of group.params) {
        if (typeof p.default === 'number') init[p.id] = p.default;
      }
    }
    return init;
  });
  const [activeGroup, setActiveGroup] = useState(PARAM_GROUPS[0].id);

  const updateParam = useCallback((id: string, value: number) => {
    setParams(prev => ({ ...prev, [id]: value }));
  }, []);

  const handleGenerate = useCallback(() => {
    onGenerate(params);
  }, [params, onGenerate]);

  const activeDef = PARAM_GROUPS.find(g => g.id === activeGroup);

  return (
    <div style={{ padding: 16, borderBottom: '1px solid #2a2a35', overflow: 'auto' }}>
      <h2 style={{ fontSize: 18, fontWeight: 600, marginBottom: 12, color: '#e8e8f0' }}>
        GeoGenesis
      </h2>

      {/* タブ */}
      <div style={{ display: 'flex', gap: 4, marginBottom: 12, flexWrap: 'wrap' }}>
        {PARAM_GROUPS.map(g => (
          <button
            key={g.id}
            onClick={() => setActiveGroup(g.id)}
            style={{
              padding: '4px 12px', fontSize: 12, border: 'none',
              borderRadius: 4, cursor: 'pointer',
              background: activeGroup === g.id ? '#4a6cf7' : '#22222e',
              color: activeGroup === g.id ? '#fff' : '#888',
              transition: 'all 0.2s',
            }}
          >
            {g.label}
          </button>
        ))}
      </div>

      {/* パラメータスライダー */}
      {activeDef?.params.map(p => {
        const val = params[p.id] ?? (typeof p.default === 'number' ? p.default : 0);
        return (
          <div key={p.id} style={{ marginBottom: 10 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', fontSize: 12, marginBottom: 2 }}>
              <span style={{ color: '#aaa' }}>{p.label}</span>
              <span style={{ color: '#4a6cf7', fontVariantNumeric: 'tabular-nums' }}>
                {p.options ? p.options[val as number] ?? val : Number(val).toExponential(2)}
              </span>
            </div>
            {p.options ? (
              <select
                value={val}
                onChange={e => updateParam(p.id, Number(e.target.value))}
                style={{
                  width: '100%', padding: '4px 8px', fontSize: 12,
                  background: '#1a1a24', color: '#e0e0e0', border: '1px solid #333',
                  borderRadius: 4,
                }}
              >
                {p.options.map((opt, i) => (
                  <option key={opt} value={i}>{opt}</option>
                ))}
              </select>
            ) : (
              <input
                type="range"
                min={p.min ?? 0}
                max={p.max ?? 1}
                step={p.step ?? 1}
                value={val}
                onChange={e => updateParam(p.id, parseFloat(e.target.value))}
                style={{ width: '100%', accentColor: '#4a6cf7' }}
              />
            )}
          </div>
        );
      })}

      {/* 生成ボタン */}
      <button
        onClick={handleGenerate}
        disabled={loading}
        style={{
          width: '100%', padding: '10px 0', marginTop: 8,
          border: 'none', borderRadius: 6, cursor: loading ? 'not-allowed' : 'pointer',
          background: loading ? '#333' : '#4a6cf7',
          color: '#fff', fontSize: 14, fontWeight: 600,
          transition: 'all 0.2s',
        }}
      >
        {loading ? 'Generating...' : 'Generate Terrain'}
      </button>
    </div>
  );
};

export default ParameterPanel;
