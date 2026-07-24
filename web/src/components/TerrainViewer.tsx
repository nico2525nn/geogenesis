import React, { useMemo, useRef } from 'react';
import { Canvas, useFrame, useThree } from '@react-three/fiber';
import { OrbitControls } from '@react-three/drei';
import * as THREE from 'three';

interface Props {
  heightmap: Float64Array | null;
  width: number;
  height: number;
}

/** 標高マップから Three.js Geometry を構築 */
function buildGeometry(data: Float64Array, w: number, h: number): THREE.BufferGeometry {
  const geo = new THREE.BufferGeometry();
  const vertices: number[] = [];
  const normals: number[] = [];
  const colors: number[] = [];
  const indices: number[] = [];

  const scale = 0.05;
  const vScale = 0.01;

  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const z = data[y * w + x] * vScale;
      vertices.push(
        (x - w / 2) * scale,
        (y - h / 2) * scale,
        z
      );

      // 標高に応じた色分け
      const normalized = (z * 5 + 1) / 2;
      const r = Math.min(1, Math.max(0, normalized * 1.2));
      const g = Math.min(1, Math.max(0, (1 - Math.abs(normalized - 0.3)) * 0.8));
      const b = Math.min(1, Math.max(0, (1 - normalized) * 1.5));
      colors.push(r, g, b);
    }
  }

  for (let y = 0; y < h - 1; y++) {
    for (let x = 0; x < w - 1; x++) {
      const i0 = y * w + x;
      const i1 = i0 + 1;
      const i2 = (y + 1) * w + x;
      const i3 = i2 + 1;
      indices.push(i0, i1, i3, i0, i3, i2);
    }
  }

  geo.setAttribute('position', new THREE.Float32BufferAttribute(vertices, 3));
  geo.setAttribute('color', new THREE.Float32BufferAttribute(colors, 3));
  geo.setIndex(indices);
  geo.computeVertexNormals();
  return geo;
}

/** 地形メッシュ */
const TerrainMesh: React.FC<{ data: Float64Array; w: number; h: number }> = ({ data, w, h }) => {
  const geo = useMemo(() => buildGeometry(data, w, h), [data, w, h]);
  const meshRef = useRef<THREE.Mesh>(null);

  useFrame(() => {
    if (meshRef.current) {
      // 自動回転オフ（OrbitControls で操作）
    }
  });

  return (
    <mesh ref={meshRef} geometry={geo}>
      <meshStandardMaterial
        vertexColors
        roughness={0.6}
        metalness={0.1}
        side={THREE.DoubleSide}
        wireframe={false}
      />
    </mesh>
  );
};

/** シーン設定 */
const SceneSetup: React.FC = () => {
  const { scene } = useThree();
  React.useEffect(() => {
    scene.background = new THREE.Color(0x0a0a0f);
    scene.fog = new THREE.Fog(0x0a0a0f, 50, 100);
  }, [scene]);
  return null;
};

/** 3D 地形ビューア */
const TerrainViewer: React.FC<Props> = ({ heightmap, width, height }) => {
  const hasData = heightmap && width > 0 && height > 0;

  return (
    <Canvas
      camera={{ position: [0, 0, 30], fov: 50, near: 0.1, far: 200 }}
      style={{ width: '100%', height: '100%' }}
    >
      <SceneSetup />
      <ambientLight intensity={0.4} />
      <directionalLight position={[10, 20, 10]} intensity={1.0} />
      <directionalLight position={[-10, 5, -10]} intensity={0.3} color="#4466aa" />

      {hasData && (
        <TerrainMesh data={heightmap!} w={width} h={height} />
      )}

      <OrbitControls
        enablePan={true}
        enableZoom={true}
        enableRotate={true}
        target={[0, 0, 0]}
        maxPolarAngle={Math.PI / 2}
      />
    </Canvas>
  );
};

export default TerrainViewer;
