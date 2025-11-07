/**
 * Test utilities for Solana Core Bridge
 */

import { PublicKey } from "@solana/web3.js";

/**
 * Helper function to create guardian addresses (Ethereum format - 20 bytes)
 */
export function createGuardians(count: number): number[][] {
  const guardians: number[][] = [];
  for (let i = 0; i < count; i++) {
    const guardian = new Array(20).fill(i + 1);
    guardians.push(guardian);
  }
  return guardians;
}

/**
 * Convert a number array to hex string
 */
export function toHex(arr: number[]): string {
  return "0x" + Buffer.from(arr).toString("hex");
}

/**
 * Create a VAA hash (simplified for testing)
 */
export function createVAAHash(
  timestamp: number,
  nonce: number,
  emitterChain: number,
  emitterAddress: number[],
  sequence: bigint,
  consistencyLevel: number,
  payload: number[]
): Buffer {
  // This is a simplified version - real implementation would use keccak256
  const data = Buffer.concat([
    Buffer.from(timestamp.toString()),
    Buffer.from(nonce.toString()),
    Buffer.from(emitterChain.toString()),
    Buffer.from(emitterAddress),
    Buffer.from(sequence.toString()),
    Buffer.from([consistencyLevel]),
    Buffer.from(payload),
  ]);
  
  return Buffer.from(data).slice(0, 32); // Simplified hash
}

/**
 * Sleep for a given number of milliseconds
 */
export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Constants
 */
export const SOLANA_CHAIN_ID = 2;
export const ETHEREUM_CHAIN_ID = 1;
export const FINALIZED_CONSISTENCY_LEVEL = 200;

