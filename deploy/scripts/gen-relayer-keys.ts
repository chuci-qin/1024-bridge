#!/usr/bin/env npx ts-node
/**
 * 生成 E2S Relayer 双密钥对（EVM + SVM）
 *
 * 每个 relayer 需要：
 * - EVM：ECDSA 私钥 → 用于在 EVM 链上签名（监听/验证）
 * - SVM：Ed25519 密钥对 → 用于在 1024/SVM 链上提交交易
 *
 * 用法：
 *   npx ts-node gen-relayer-keys.ts [--name relayer1] [--count 1] [--out relayers.json]
 *
 * 不传 --out 时只打印到 stdout，便于复制到 .env 和 deploy/keys/relayers.json
 */

import { Keypair } from "@solana/web3.js";
import { Wallet } from "ethers";
import * as fs from "fs";
import * as path from "path";

interface RelayerKeys {
  name: string;
  evm_address: string;
  evm_private_key: string;
  svm_pubkey: string;
  svm_private_key_comma: string; // 32 字节种子逗号分隔数字，用于 RELAYER_ED25519_PRIVATE_KEY；Rust 侧取前 32 字节 from_seed
}

function generateOne(name: string): RelayerKeys {
  const evmWallet = Wallet.createRandom();
  const svmKeypair = Keypair.generate();
  const seed = Array.from(svmKeypair.secretKey.slice(0, 32));
  const seedList = seed.join(",");

  return {
    name,
    evm_address: evmWallet.address,
    evm_private_key: evmWallet.privateKey,
    svm_pubkey: svmKeypair.publicKey.toBase58(),
    svm_private_key_comma: seedList,
  };
}

function main() {
  const args = process.argv.slice(2);
  let name = "relayer1";
  let count = 1;
  let outFile: string | null = null;
  let svmOnly = false;

  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--name" && args[i + 1]) {
      name = args[++i];
    } else if (args[i] === "--count" && args[i + 1]) {
      count = parseInt(args[++i], 10) || 1;
    } else if (args[i] === "--out" && args[i + 1]) {
      outFile = args[++i];
    } else if (args[i] === "--svm-only") {
      svmOnly = true;
    } else if (args[i] === "--help" || args[i] === "-h") {
      console.log(`
gen-relayer-keys.ts — 生成 E2S Relayer 的 EVM + SVM 密钥对

用法:
  npx ts-node gen-relayer-keys.ts [选项]

选项:
  --name <前缀>    relayer 名称前缀，默认 relayer1（多份时为 relayer1, relayer2, ...）
  --count <N>      生成 N 个 relayer，默认 1
  --out <路径>     将 relayers 写入 JSON，并在同目录生成 <文件名>-secrets.txt（含私钥，可复制到 .env）
  --svm-only       仅 SVM（deploy1 用）：JSON 只含 name/svm_pubkey，secrets 只含 RELAYER_ED25519_PRIVATE_KEY
  -h, --help       显示此帮助

输出:
  - 使用 --out 时：写入 <out> 的 JSON，以及同目录下的 <out  basename>-secrets.txt
  - 同时打印到 stdout，便于复制
`);
      process.exit(0);
    }
  }

  const relayers: RelayerKeys[] = [];
  for (let i = 0; i < count; i++) {
    const n = count > 1 ? `${name.replace(/\d+$/, "")}${i + 1}` : name;
    relayers.push(generateOne(n));
  }

  const outDir = outFile ? path.dirname(path.resolve(outFile)) : null;
  if (outFile && outDir) {
    fs.mkdirSync(outDir, { recursive: true });
    const forDeploy = {
      relayers: relayers.map((r) =>
        svmOnly
          ? { name: r.name, svm_pubkey: r.svm_pubkey }
          : { name: r.name, evm_address: r.evm_address, svm_pubkey: r.svm_pubkey }
      ),
    };
    fs.writeFileSync(outFile, JSON.stringify(forDeploy, null, 2), "utf-8");
    console.log("Wrote", path.resolve(outFile));

    const base = path.basename(outFile, path.extname(outFile));
    const secretsPath = path.join(outDir, `${base}-secrets.txt`);
    const secretsLines: string[] = [];
    if (svmOnly) {
      secretsLines.push("# deploy1：Sol↔SVM 桥，仅 SVM relayer（无 EVM）", "");
    } else {
      secretsLines.push("# 以下为各 relayer 的私钥，请妥善保存并填入对应 .env（勿提交）", "");
    }
    for (const r of relayers) {
      if (svmOnly) {
        secretsLines.push(`# ${r.name} (svm_pubkey: ${r.svm_pubkey})`);
        secretsLines.push("RELAYER_ED25519_PRIVATE_KEY=" + r.svm_private_key_comma);
      } else {
        secretsLines.push(`# ${r.name}`);
        secretsLines.push("RELAYER_ECDSA_PRIVATE_KEY=" + r.evm_private_key);
        secretsLines.push("RELAYER_ED25519_PRIVATE_KEY=" + r.svm_private_key_comma);
      }
      secretsLines.push("");
    }
    fs.writeFileSync(secretsPath, secretsLines.join("\n"), "utf-8");
    console.log("Wrote", secretsPath);
    console.log(
      svmOnly
        ? "\n# deploy1：Sol↔SVM 桥，仅 SVM relayer（无 EVM）\n"
        : "\n# 以下为各 relayer 的私钥，请妥善保存并填入对应 .env（勿提交）\n"
    );
  }

  for (const r of relayers) {
    if (svmOnly) {
      console.log(`# ${r.name} (svm_pubkey: ${r.svm_pubkey})`);
      console.log("RELAYER_ED25519_PRIVATE_KEY=" + r.svm_private_key_comma);
    } else {
      console.log(`# ${r.name}`);
      console.log("RELAYER_ECDSA_PRIVATE_KEY=" + r.evm_private_key);
      console.log("RELAYER_ED25519_PRIVATE_KEY=" + r.svm_private_key_comma);
    }
    console.log("");
  }
}

main();
