declare module 'geogenesis-wasm' {
  export class GeoGenesisEngine {
    constructor();
    init(worldJson: string, configJson: string): void;
    generateInitial(): void;
    step(dt: number): void;
    run(): void;
    getHeightmap(): number[];
    getWidth(): number;
    getHeight(): number;
    getStatistics(): string;
    getCurrentTime(): number;
  }
}
