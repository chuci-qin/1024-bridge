import { Router, Request, Response } from 'express';
import { executeWithdraw, WithdrawResult } from '../services/lifi';
import { createRateLimiter, createRequestQueue } from '../utils/rateLimiter';
import { config } from '../config';

const router = Router();

// 创建速率限制器和请求队列
const rateLimiter = createRateLimiter(!!config.lifi.apiKey);
const requestQueue = createRequestQueue(5);

/**
 * POST /withdraw
 * 接收提现请求，调用 LiFi SDK 发起跨链交易
 */
router.post('/', async (req: Request, res: Response) => {
  try {
    const { target_chain, target_asset, usdc_amount, recipient_address, proof } = req.body;

    // 参数验证
    if (!target_chain || !target_asset || !usdc_amount || !recipient_address) {
      return res.status(400).json({
        success: false,
        message: 'Missing required parameters: target_chain, target_asset, usdc_amount, recipient_address',
        route_id: null,
        tx_hash: null,
      });
    }

    // 基本的 proof 存在性检查（后续可扩展为完整的 WithdrawProof 结构与链上验证）
    if (!proof) {
      return res.status(400).json({
        success: false,
        message: 'Missing required parameter: proof',
        route_id: null,
        tx_hash: null,
      });
    }

    // 验证参数类型
    if (typeof target_chain !== 'number') {
      return res.status(400).json({
        success: false,
        message: 'target_chain must be a number',
        route_id: null,
        tx_hash: null,
      });
    }

    if (typeof target_asset !== 'string' || !target_asset.startsWith('0x')) {
      return res.status(400).json({
        success: false,
        message: 'target_asset must be a valid hex address (starting with 0x)',
        route_id: null,
        tx_hash: null,
      });
    }

    if (typeof usdc_amount !== 'string') {
      return res.status(400).json({
        success: false,
        message: 'usdc_amount must be a string',
        route_id: null,
        tx_hash: null,
      });
    }

    if (typeof recipient_address !== 'string' || !recipient_address.startsWith('0x')) {
      return res.status(400).json({
        success: false,
        message: 'recipient_address must be a valid hex address (starting with 0x)',
        route_id: null,
        tx_hash: null,
      });
    }

    // TODO(security): 在这里调用 verifyWithdrawProof(proof, { target_chain, usdc_amount, recipient_address })
    // 用于：
    // - 验证 Solana/1024chain 钱包签名
    // - 验证 1024chain 上 stake 交易是否存在且成功
    // - 验证金额 / nonce / 接收地址是否与 proof 一致
    // - 原子防重放（proofId 一次性消费）

    // 检查速率限制
    try {
      await rateLimiter.consume('lifi-api');
    } catch (error: any) {
      if (error.remainingPoints !== undefined) {
        return res.status(429).json({
          success: false,
          message: 'Rate limit exceeded. Please try again later.',
          route_id: null,
          tx_hash: null,
        });
      }
      throw error;
    }

    // 加入请求队列（控制并发）
    const result = await requestQueue.add(async (): Promise<WithdrawResult> => {
      return await executeWithdraw({
        targetChain: target_chain,
        targetAsset: target_asset,
        usdcAmount: usdc_amount,
        recipientAddress: recipient_address,
      });
    });

    if (!result) {
      throw new Error('Failed to execute withdraw');
    }

    res.json({
      success: true,
      message: 'Withdrawal initiated',
      route_id: result.routeId,
      tx_hash: result.txHash || null,
    });
  } catch (error: any) {
    console.error('Withdraw error:', error);
    res.status(500).json({
      success: false,
      message: error.message || 'Internal server error',
      route_id: null,
      tx_hash: null,
    });
  }
});

export default router;

