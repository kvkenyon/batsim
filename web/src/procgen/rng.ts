/**
 * Deterministic PRNG for world layout. The same seed must produce the same
 * streets and home placements on every machine, so nothing here may touch
 * Math.random: all randomness derives from splitmix64 over string hashes.
 */

/** FNV-1a 64-bit hash, folded to a 53-bit safe integer. */
export function hashString(input: string): bigint {
  let h = 0xcbf29ce484222325n;
  for (let i = 0; i < input.length; i++) {
    h ^= BigInt(input.charCodeAt(i));
    h = BigInt.asUintN(64, h * 0x100000001b3n);
  }
  return h;
}

/** splitmix64 step: state in, (new state, uniform double in [0,1)) out. */
export function splitmix64Next(state: bigint): [bigint, number] {
  let z = BigInt.asUintN(64, state + 0x9e3779b97f4a7c15n);
  z = BigInt.asUintN(64, (z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n);
  z = BigInt.asUintN(64, (z ^ (z >> 27n)) * 0x94d049bb133111ebn);
  z = z ^ (z >> 31n);
  return [z, Number(z >> 11n) / 0x20000000000000];
}

/** A small seeded stream of uniform doubles. */
export class Rng {
  private state: bigint;

  constructor(seed: string | bigint) {
    this.state = typeof seed === "string" ? hashString(seed) : BigInt.asUintN(64, seed);
  }

  next(): number {
    const [state, value] = splitmix64Next(this.state);
    this.state = state;
    return value;
  }

  /** Uniform in [min, max). */
  range(min: number, max: number): number {
    return min + this.next() * (max - min);
  }

  /** Uniform integer in [0, n). */
  int(n: number): number {
    return Math.floor(this.next() * n);
  }
}
