import React from 'react';
import type { TerrainStatistics } from '../types';

interface Props {
  stats: TerrainStatistics;
}

const StatRow: React.FC<{ label: string; value: string }> = ({ label, value }) => (
  <div style={{ display: 'flex', justifyContent: 'space-between', padding: '4px 0', fontSize: 13 }}>
    <span style={{ color: '#888' }}>{label}</span>
    <span style={{ color: '#e0e0e0', fontVariantNumeric: 'tabular-nums' }}>{value}</span>
  </div>
);

const TerrainAnalysis: React.FC<Props> = ({ stats }) => {
  return (
    <div style={{ padding: 16, overflow: 'auto', flex: 1 }}>
      <h3 style={{ fontSize: 14, fontWeight: 600, marginBottom: 12, color: '#e8e8f0' }}>
        Terrain Analysis
      </h3>

      <div style={{ marginBottom: 16 }}>
        <h4 style={{ fontSize: 12, color: '#666', marginBottom: 4, textTransform: 'uppercase', letterSpacing: 1 }}>
          Elevation
        </h4>
        <StatRow label="Min" value={`${stats.minElevation.toFixed(1)} m`} />
        <StatRow label="Max" value={`${stats.maxElevation.toFixed(1)} m`} />
        <StatRow label="Mean" value={`${stats.meanElevation.toFixed(1)} m`} />
        <StatRow label="Std Dev" value={`${stats.stdDev.toFixed(1)} m`} />
        <StatRow label="Median" value={`${stats.median.toFixed(1)} m`} />
      </div>

      <div style={{ marginBottom: 16 }}>
        <h4 style={{ fontSize: 12, color: '#666', marginBottom: 4, textTransform: 'uppercase', letterSpacing: 1 }}>
          Grid
        </h4>
        <StatRow label="Total Cells" value={stats.totalCells.toLocaleString()} />
      </div>

      <div style={{
        marginTop: 16, padding: 12, background: '#1a1a24', borderRadius: 8,
        fontSize: 12, color: '#666', lineHeight: 1.6,
      }}>
        <strong style={{ color: '#888' }}>Range:</strong><br />
        {stats.minElevation.toFixed(0)} – {stats.maxElevation.toFixed(0)} m
        <div style={{
          marginTop: 8, height: 4, borderRadius: 2,
          background: 'linear-gradient(to right, #1a3a5c, #4a8f4a, #8a6a3a, #c8b88a, #ffffff)',
          width: '100%',
        }} />
      </div>
    </div>
  );
};

export default TerrainAnalysis;
