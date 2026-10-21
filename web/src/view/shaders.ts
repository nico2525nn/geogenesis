// GLSL programs for the terrain viewport.
//
// The terrain mesh is a *static* grid: the vertex shader displaces it from an
// R32F height texture and derives smooth normals from the same texture, so a
// simulation frame only costs one small texture upload. Albedo is evaluated
// per fragment from linear-light lookup tables (see `palettes.ts`), which makes
// mode changes and overlay-field arrivals free of mesh rebuilds.
//
// Lighting is hand-written rather than MeshStandardMaterial so we can afford
// warm sun / cool sky wrap lighting, ridge self-shadowing, curvature ambient
// occlusion and aerial perspective in one pass.

/** Shared value-noise / hash helpers used by the terrain, water and skirt. */
const NOISE = /* glsl */ `
float hash12(vec2 p) {
  vec3 p3 = fract(vec3(p.xyx) * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}

float valueNoise(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  vec2 u = f * f * (3.0 - 2.0 * f);
  float a = hash12(i);
  float b = hash12(i + vec2(1.0, 0.0));
  float c = hash12(i + vec2(0.0, 1.0));
  float d = hash12(i + vec2(1.0, 1.0));
  return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

float fbm(vec2 p) {
  return valueNoise(p) * 0.58 + valueNoise(p * 2.63 + 17.0) * 0.28 + valueNoise(p * 5.7 + 4.0) * 0.14;
}
`;

/** Aerial-perspective blend shared by every opaque material. */
const HAZE = /* glsl */ `
vec3 hazeColour(vec3 worldPos, vec3 sunDir, vec3 hazeCool, vec3 hazeWarm) {
  vec3 viewDir = normalize(worldPos - cameraPosition);
  float glow = pow(clamp(dot(viewDir, sunDir), 0.0, 1.0), 4.0);
  return mix(hazeCool, hazeWarm, glow);
}

float hazeFactor(float distanceToCamera, float fogDensity) {
  float d = distanceToCamera * fogDensity;
  return 1.0 - exp(-d * d);
}
`;

// ---------------------------------------------------------------------------
// Terrain
// ---------------------------------------------------------------------------

export const TERRAIN_VERT = /* glsl */ `
uniform sampler2D uHeight;
uniform vec2 uGrid;
uniform float uWorldWidth;
uniform float uWorldDepth;
uniform float uUnitsPerMetre;
uniform float uSeaLevelM;
uniform float uAOScale;

varying vec3 vWorldPos;
varying vec2 vGridUv;
varying vec3 vNormal;
varying float vElevationM;
varying vec2 vShade; // x: curvature ambient occlusion, y: ridge exposure

float heightAt(vec2 uv) {
  return texture2D(uHeight, uv).r;
}

void main() {
  vec2 texel = 1.0 / uGrid;
  float e = heightAt(uv);
  float eL = heightAt(vec2(uv.x - texel.x, uv.y));
  float eR = heightAt(vec2(uv.x + texel.x, uv.y));
  float eD = heightAt(vec2(uv.x, uv.y - texel.y));
  float eU = heightAt(vec2(uv.x, uv.y + texel.y));

  float h = (e - uSeaLevelM) * uUnitsPerMetre;
  vec3 displaced = vec3(position.x, h, position.z);

  float dx = uWorldWidth / max(uGrid.x - 1.0, 1.0);
  float dz = uWorldDepth / max(uGrid.y - 1.0, 1.0);
  vec3 normal = normalize(vec3(
    -(eR - eL) * uUnitsPerMetre / (2.0 * dx),
    1.0,
    -(eU - eD) * uUnitsPerMetre / (2.0 * dz)
  ));

  // Curvature-based ambient occlusion: valleys collect shade, ridges catch
  // light. Computed from the same four taps as the normal, so it is free.
  float concavity = (eL + eR + eD + eU) * 0.25 - e;
  float ao = 1.0 - clamp(concavity / uAOScale, 0.0, 0.55);
  float ridge = clamp(-concavity / (uAOScale * 2.5), 0.0, 1.0);

  vec4 worldPos = modelMatrix * vec4(displaced, 1.0);
  vWorldPos = worldPos.xyz;
  vGridUv = uv;
  vNormal = normal;
  vElevationM = e;
  vShade = vec2(clamp(ao, 0.35, 1.1), ridge);

  gl_Position = projectionMatrix * viewMatrix * worldPos;
}
`;

export const TERRAIN_FRAG = /* glsl */ `
uniform sampler2D uLandLut;
uniform sampler2D uOceanLut;
uniform sampler2D uModeLut;
uniform sampler2D uField;
uniform float uHasField;
uniform int uModeKind; // 0 terrain, 1 categorical, 2 continuous
uniform float uCatCount;
uniform vec2 uLutSize; // x: land size, y: ocean size

uniform vec3 uSunDir;
uniform vec3 uSunColour;
uniform vec3 uSkyColour;
uniform vec3 uGroundColour;
uniform vec3 uHazeCool;
uniform vec3 uHazeWarm;
uniform float uFogDensity;
uniform float uTime;
uniform float uReliefShade;
uniform float uSeaLevelM;

varying vec3 vWorldPos;
varying vec2 vGridUv;
varying vec3 vNormal;
varying float vElevationM;
varying vec2 vShade;

${NOISE}
${HAZE}

vec3 terrainAlbedo(float elevationM) {
  float above = (elevationM - uSeaLevelM) / 2600.0;
  if (above >= 0.0) {
    float t = clamp(above, 0.0, 1.0) * (uLutSize.x - 1.0);
    return texture2D(uLandLut, vec2((floor(t) + 0.5) / uLutSize.x, 0.5)).rgb;
  }
  float depth = (uSeaLevelM - elevationM) / 6000.0;
  float t = clamp(depth, 0.0, 1.0) * (uLutSize.y - 1.0);
  return texture2D(uOceanLut, vec2((floor(t) + 0.5) / uLutSize.y, 0.5)).rgb;
}

vec3 modeAlbedo() {
  if (uHasField < 0.5) {
    return terrainAlbedo(vElevationM);
  }
  float encoded = texture2D(uField, vGridUv).r;
  if (uModeKind == 1) {
    float id = floor(encoded * 255.0 + 0.5);
    float u = (clamp(id, 0.0, uCatCount - 1.0) + 0.5) / uCatCount;
    return texture2D(uModeLut, vec2(u, 0.5)).rgb;
  }
  float t = clamp(encoded, 0.0, 1.0) * (uLutSize.x - 1.0);
  return texture2D(uModeLut, vec2((floor(t) + 0.5) / uLutSize.x, 0.5)).rgb;
}

void main() {
  vec3 normal = normalize(vNormal);
  vec3 viewDir = normalize(cameraPosition - vWorldPos);
  float distanceToCamera = distance(cameraPosition, vWorldPos);
  bool terrainMode = uModeKind == 0;

  vec3 albedo = terrainMode ? terrainAlbedo(vElevationM) : modeAlbedo();

  if (terrainMode) {
    // Two scales of surface grain break up the smooth hypsometric ramp; the
    // fine octave fades with distance so it never shimmers.
    float grain = fbm(vWorldPos.xz * 0.16) - 0.5;
    float fine = valueNoise(vWorldPos.xz * 0.62) - 0.5;
    float nearFade = 1.0 - smoothstep(110.0, 300.0, distanceToCamera);
    float flatness = smoothstep(0.15, 0.6, normal.y); // noise fades on walls
    albedo *= 1.0 + (grain * 0.085 + fine * 0.025 * nearFade) * flatness;
    // Slightly richer palette in 3D than in the flat legend swatch.
    float albedoLuma = dot(albedo, vec3(0.2126, 0.7152, 0.0722));
    albedo = mix(vec3(albedoLuma), albedo, 1.14);
    // Exposed rock on steep faces: neutral stone tones instead of muddy
    // tinted versions of the vegetation colour, modulated so it looks broken.
    float steep = smoothstep(0.5, 0.9, 1.0 - normal.y);
    float rockPatches = 0.5 + 0.5 * fbm(vWorldPos.xz * 0.3 + 7.1);
    vec3 stone = mix(vec3(albedoLuma), vec3(0.66, 0.64, 0.62), 0.4) * 0.9;
    albedo = mix(albedo, stone, steep * rockPatches * 0.5);
    // Blue-ish bounce from the sky in crevices; keeps snow from going flat.
    float snowiness = smoothstep(0.58, 0.82, albedoLuma);
    albedo = mix(albedo, albedo * vec3(0.84, 0.90, 1.02), (1.0 - vShade.x) * 2.2 * snowiness * 0.5);
    albedo = mix(albedo, albedo * vec3(0.90, 0.94, 1.03), steep * 0.2);
    // Damp, darker sediment right at the waterline softens the coast edge.
    float shoreWet = 1.0 - smoothstep(0.0, 14.0, vElevationM - uSeaLevelM);
    albedo = mix(albedo, albedo * vec3(0.62, 0.66, 0.66) + vec3(0.012, 0.014, 0.012), shoreWet * 0.55);
    // Snow surface variation: wind-blown streaks keep white fields from
    // reading as one flat blob.
    float snowMask = smoothstep(0.6, 0.85, albedoLuma);
    albedo *= 1.0 + (valueNoise(vWorldPos.xz * 0.32 + 11.0) - 0.5) * 0.16 * snowMask;
  }

  float ndl = dot(normal, uSunDir);
  float sunTerm = smoothstep(-0.14, 0.42, ndl);
  vec3 sunlight = uSunColour * sunTerm * (1.0 + vShade.y * 0.14);
  float skyVis = clamp(0.32 + 0.68 * normal.y, 0.0, 1.0) * vShade.x;
  vec3 ambient = uSkyColour * skyVis;
  vec3 bounce = uGroundColour * clamp(0.45 - 0.45 * normal.y, 0.0, 1.0);
  vec3 light = sunlight + ambient + bounce;

  vec3 colour;
  if (terrainMode) {
    colour = albedo * light;
    // Broad rough specular sheen on snow / ice.
    vec3 halfway = normalize(uSunDir + viewDir);
    float spec = pow(max(dot(normal, halfway), 0.0), 48.0);
    float luma = dot(albedo, vec3(0.2126, 0.7152, 0.0722));
    colour += uSunColour * spec * smoothstep(0.42, 0.78, luma) * 0.45 * sunTerm;
    // Cool rim on backlit ridges.
    float rim = pow(1.0 - clamp(dot(normal, viewDir), 0.0, 1.0), 3.0);
    colour += uSkyColour * rim * 0.12 * (1.0 - sunTerm);
  } else {
    // Data overlays keep their legend colours; light only shapes the terrain,
    // and is compressed so the ramps do not desaturate under the filmic curve.
    vec3 shaped = clamp(light, vec3(0.62), vec3(1.3));
    colour = albedo * mix(vec3(1.0), shaped, uReliefShade);
    colour *= 0.98;
  }

  // Aerial perspective.
  float fog = hazeFactor(distanceToCamera, uFogDensity);
  float lowland = 1.0 - smoothstep(-1.0, 16.0, vWorldPos.y);
  fog = clamp(fog * (0.62 + 0.38 * lowland), 0.0, 0.94);
#ifdef GG_DATA_MODE
  // Data overlays must stay faithful to the legend; only a hint of haze.
  fog *= 0.3;
  float dataLuma = dot(colour, vec3(0.2126, 0.7152, 0.0722));
  colour = mix(vec3(dataLuma), colour, 1.05);
#endif
  vec3 haze = hazeColour(vWorldPos, uSunDir, uHazeCool, uHazeWarm);
  colour = mix(colour, haze, fog);

  gl_FragColor = vec4(colour, 1.0);
#ifdef GG_DATA_MODE
  #include <colorspace_fragment>
#else
  #include <tonemapping_fragment>
  #include <colorspace_fragment>
#endif
}
`;

// ---------------------------------------------------------------------------
// Water
// ---------------------------------------------------------------------------

export const WATER_VERT = /* glsl */ `
varying vec3 vWorldPos;
varying vec2 vUv;

void main() {
  vUv = uv;
  vec4 worldPos = modelMatrix * vec4(position, 1.0);
  vWorldPos = worldPos.xyz;
  gl_Position = projectionMatrix * viewMatrix * worldPos;
}
`;

export const WATER_FRAG = /* glsl */ `
uniform sampler2D uHeight;
uniform vec2 uGrid;
uniform float uWorldWidth;
uniform float uWorldDepth;
uniform float uSeaLevelM;

uniform vec3 uSunDir;
uniform vec3 uSunColour;
uniform vec3 uDeep;
uniform vec3 uShallow;
uniform vec3 uFoam;
uniform vec3 uSkyTop;
uniform vec3 uSkyHorizon;
uniform vec3 uHazeCool;
uniform vec3 uHazeWarm;
uniform float uFogDensity;
uniform float uTime;
uniform float uOpacity;

varying vec3 vWorldPos;
varying vec2 vUv;

${NOISE}
${HAZE}

float heightAtUv(vec2 uv) {
  return texture2D(uHeight, uv).r;
}

float bilinearHeight(vec2 uv) {
  vec2 t = uv * uGrid - 0.5;
  vec2 f = fract(t);
  vec2 base = (t - f + 0.5) / uGrid;
  vec2 stepUv = 1.0 / uGrid;
  float h00 = heightAtUv(base);
  float h10 = heightAtUv(base + vec2(stepUv.x, 0.0));
  float h01 = heightAtUv(base + vec2(0.0, stepUv.y));
  float h11 = heightAtUv(base + stepUv);
  return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y);
}

void main() {
  // The plane's v=0 row is at +Z (see geometry construction), while the height
  // texture's row 0 is at -Z, hence the flip.
  vec2 heightUv = vec2(vUv.x, 1.0 - vUv.y);
  float elevation = bilinearHeight(heightUv);
  float depthM = max(uSeaLevelM - elevation, 0.0);

  vec3 viewDir = normalize(cameraPosition - vWorldPos);
  vec2 p = vWorldPos.xz;
  float t = uTime;

  // Gentle analytic ripples: sum of directional waves, evaluated as a slope.
  // The long swells fade with distance (they alias into streaks at grazing
  // angles); the fine chop stays to keep the surface alive up close.
  float rippleFade = 1.0 - smoothstep(120.0, 300.0, distance(cameraPosition, vWorldPos)) * 0.65;
  vec2 slope = vec2(0.0);
  slope += vec2(0.86, 0.51) * cos(dot(vec2(0.86, 0.51), p) * 0.36 + t * 0.85) * 0.013 * rippleFade;
  slope += vec2(-0.42, 0.91) * cos(dot(vec2(-0.42, 0.91), p) * 0.61 + t * 1.25) * 0.009 * rippleFade;
  slope += vec2(0.71, -0.70) * cos(dot(vec2(0.71, -0.70), p) * 1.15 + t * 1.9) * 0.007;
  slope += vec2(0.99, 0.12) * cos(dot(vec2(0.99, 0.12), p) * 2.3 + t * 2.7) * 0.004;
  float swirl = valueNoise(p * 0.9 + vec2(t * 0.11, -t * 0.07));
  slope += (vec2(valueNoise(p * 0.9 + 0.1), valueNoise(p * 0.9 + 0.17)) - swirl) * 0.035 * rippleFade;

  vec3 normal = normalize(vec3(-slope.x, 1.0, -slope.y));
  float ndv = clamp(dot(normal, viewDir), 0.0, 1.0);
  float fresnel = pow(1.0 - ndv, 4.0);

  vec3 reflected = reflect(-viewDir, normal);
  vec3 skyReflection = mix(uSkyHorizon, uSkyTop, clamp(reflected.y * 1.8, 0.0, 1.0));

  float depthT = clamp(pow(depthM / 2200.0, 0.6), 0.0, 1.0);
  vec3 body = mix(uShallow, uDeep, depthT);

  vec3 halfway = normalize(uSunDir + viewDir);
  float glint = pow(max(dot(normal, halfway), 0.0), 220.0) * 3.2;
  float sheen = pow(max(dot(normal, halfway), 0.0), 18.0) * 0.22;
  float sunWarm = max(dot(normal, uSunDir), 0.0) * 0.035;

  float shore = 1.0 - smoothstep(0.0, 70.0, depthM);
  float foamNoise = valueNoise(p * 0.5 + vec2(t * 0.22, -t * 0.16)) * 0.62
    + valueNoise(p * 1.9 - vec2(t * 0.33, t * 0.21)) * 0.38;
  float foam = smoothstep(0.5, 0.9, shore * (0.45 + 1.15 * foamNoise));
  foam *= 1.0 - smoothstep(45.0, 70.0, depthM);

  // Fresnel sky reflection plus a whisper of subsurface scatter; the small
  // base term keeps deep water from reading as a black hole.
  vec3 colour = mix(body, skyReflection, clamp(fresnel * 0.85 + 0.05, 0.0, 0.92));
  colour += uShallow * depthT * (1.0 - depthT) * 0.2;
  colour += uSunColour * (glint + sheen + sunWarm) * clamp(uSunDir.y * 2.0 + 0.2, 0.0, 1.0);
  colour = mix(colour, uFoam, clamp(foam, 0.0, 0.92));

  float alpha = mix(0.52, 0.955, depthT);
  alpha = mix(alpha, 1.0, fresnel * 0.35);
  alpha = max(alpha, foam * 0.92);
  alpha *= uOpacity;

  float distanceToCamera = distance(cameraPosition, vWorldPos);
  float fog = hazeFactor(distanceToCamera, uFogDensity) * 0.55;
  vec3 haze = hazeColour(vWorldPos, uSunDir, uHazeCool, uHazeWarm);
  colour = mix(colour, haze, fog * 0.92);
  alpha *= 1.0 - fog * 0.3;

  gl_FragColor = vec4(colour, alpha);
  #include <tonemapping_fragment>
  #include <colorspace_fragment>
}
`;

// ---------------------------------------------------------------------------
// Cliff skirt around the map edge
// ---------------------------------------------------------------------------

export const SKIRT_VERT = /* glsl */ `
varying vec3 vWorldPos;
varying vec3 vNormal;

void main() {
  vec4 worldPos = modelMatrix * vec4(position, 1.0);
  vWorldPos = worldPos.xyz;
  vNormal = normalize(mat3(modelMatrix) * normal);
  gl_Position = projectionMatrix * viewMatrix * worldPos;
}
`;

export const SKIRT_FRAG = /* glsl */ `
uniform vec3 uSunDir;
uniform vec3 uSunColour;
uniform vec3 uSkyColour;
uniform vec3 uGroundColour;
uniform vec3 uHazeCool;
uniform vec3 uHazeWarm;
uniform float uFogDensity;
uniform float uBaseY;
uniform vec3 uStrataA;
uniform vec3 uStrataB;
uniform vec3 uStrataC;

varying vec3 vWorldPos;
varying vec3 vNormal;

${NOISE}
${HAZE}

void main() {
  vec3 normal = normalize(vNormal);
  vec3 viewDir = normalize(cameraPosition - vWorldPos);
  float ndl = dot(normal, uSunDir);
  float wrap = clamp(ndl * 0.6 + 0.4, 0.0, 1.0);
  vec3 light = uSunColour * pow(wrap, 1.5) * 0.9
    + uSkyColour * (0.95 + 0.3 * normal.y)
    + uGroundColour * 0.16
    + vec3(0.035, 0.035, 0.04);

  // Layered sedimentary column: discrete beds with bedding-plane shadows,
  // gently folded along the map edges.
  float fold = (valueNoise(vec2(vWorldPos.x + vWorldPos.z, vWorldPos.y * 0.3) * 0.55) - 0.5) * 1.3
    + (valueNoise(vec2(vWorldPos.x - vWorldPos.z, vWorldPos.y * 0.14) * 0.3) - 0.5) * 1.1;
  float coord = (vWorldPos.y + fold) / 2.9;
  float layer = floor(coord);
  float f = fract(coord);
  float h1 = hash12(vec2(layer, 1.7));
  float h2 = hash12(vec2(layer, 9.3));

  vec3 strata = mix(uStrataA, uStrataB, step(0.5, h1));
  strata = mix(strata, uStrataC, step(0.8, h2));
  strata *= 0.82 + 0.36 * h1;

  // Bedding planes: a dark seam at the bottom of each bed.
  float seam = smoothstep(0.0, 0.10, f);
  strata *= mix(0.66, 1.0, seam);
  // Vertical weathering fractures.
  float fracture = smoothstep(0.6, 0.96,
    valueNoise(vec2((vWorldPos.x + vWorldPos.z) * 1.6, vWorldPos.y * 0.16)));
  strata *= 1.0 - fracture * 0.24;

  float depthFade = clamp((vWorldPos.y - uBaseY) / 18.0, 0.0, 1.0);
  vec3 colour = strata * light * mix(0.32, 1.0, depthFade);

  float distanceToCamera = distance(cameraPosition, vWorldPos);
  float fog = hazeFactor(distanceToCamera, uFogDensity);
  vec3 haze = hazeColour(vWorldPos, uSunDir, uHazeCool, uHazeWarm);
  colour = mix(colour, haze, fog * 0.95);

  gl_FragColor = vec4(colour, 1.0);
  #include <tonemapping_fragment>
  #include <colorspace_fragment>
}
`;
// ---------------------------------------------------------------------------
// Sky dome
// ---------------------------------------------------------------------------

export const SKY_VERT = /* glsl */ `
varying vec3 vDirection;

void main() {
  vDirection = position;
  vec4 mvPosition = modelViewMatrix * vec4(position, 1.0);
  gl_Position = projectionMatrix * mvPosition;
  // Pin the dome to the far plane so it always fills the background.
  gl_Position.z = gl_Position.w;
}
`;

export const SKY_FRAG = /* glsl */ `
uniform vec3 uSunDir;
uniform vec3 uZenith;
uniform vec3 uMidSky;
uniform vec3 uHorizon;
uniform vec3 uVoid;
uniform vec3 uSunGlow;
uniform float uTime;

varying vec3 vDirection;

${NOISE}

void main() {
  vec3 dir = normalize(vDirection);
  float y = dir.y;

  vec3 skyAbove = mix(uHorizon, uMidSky, smoothstep(0.0, 0.18, y));
  skyAbove = mix(skyAbove, uZenith, smoothstep(0.14, 0.62, y));
  vec3 skyBelow = mix(uHorizon, uVoid, smoothstep(0.0, 0.26, -y));
  vec3 colour = mix(skyBelow, skyAbove, smoothstep(-0.014, 0.012, y));

  // Warm scattering towards the sun azimuth, hugging the horizon.
  float sunAmount = clamp(dot(dir, uSunDir), 0.0, 1.0);
  float warm = pow(sunAmount, 4.0) * (0.16 + 0.30 * exp(-abs(y) * 5.0));
  colour += uSunGlow * warm;

  // A very faint star field in the upper dome.
  vec3 starCell = floor(dir * 170.0);
  float star = step(0.9976, hash12(starCell.xy + starCell.z * 13.0));
  float twinkle = 0.55 + 0.45 * sin(uTime * 1.7 + hash12(starCell.zx) * 30.0);
  colour += vec3(0.7, 0.78, 1.0) * star * twinkle * smoothstep(0.2, 0.6, y) * 0.25;

  gl_FragColor = vec4(colour, 1.0);
  #include <tonemapping_fragment>
  #include <colorspace_fragment>
}
`;
