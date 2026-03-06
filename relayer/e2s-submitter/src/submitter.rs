use crate::config::SubmitterConfig;
use crate::signer::Ed25519Signer;
use anyhow::{anyhow, Result};
use shared::types::{StakeEventData, CompactStakeEventData};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Signer,
    system_program,
    sysvar,
    transaction::Transaction,
};
use std::{path::Path, str::FromStr};
use tracing::{error, info, warn};
use hex;
use bs58;

/// 错误类型分类
#[derive(Debug, Clone, Copy, PartialEq)]
enum ErrorCategory {
    /// 可重试错误：网络错误、RPC超时、gas估算失败等
    Retryable,
    /// 不可重试错误：InvalidNonce、签名验证失败、权限错误等
    NonRetryable,
}

/// 启动事件处理器
pub async fn start_processor(config: SubmitterConfig) -> Result<()> {
    info!("Starting event processor");
    
    // 创建签名器和 RPC 客户端
    let private_key = config
        .relayer
        .ed25519_private_key
        .as_deref()
        .ok_or_else(|| anyhow!("Ed25519 private key not configured"))?;
    let signer = Ed25519Signer::new(private_key)?;
    let rpc_client = RpcClient::new_with_commitment(
        config.target_chain.rpc_url.clone(),
        CommitmentConfig::confirmed(),
    );
    let program_id = Pubkey::from_str(&config.target_chain.contract_address)?;
    
    // 解析 USDC mint 地址
    let usdc_mint = config
        .target_chain
        .usdc_mint
        .as_ref()
        .and_then(|m| Pubkey::from_str(m).ok())
        .ok_or_else(|| anyhow!("USDC mint address not configured in TARGET_CHAIN__USDC_MINT"))?;
    
    // 自动检测 USDC mint 使用的 token 程序（SPL Token 或 Token-2022）
    let mint_account = rpc_client
        .get_account(&usdc_mint)
        .map_err(|e| anyhow!("Failed to fetch USDC mint account: {}", e))?;
    let token_program_id = mint_account.owner;
    
    info!(
        relayer_pubkey = %signer.keypair().pubkey(),
        program_id = %program_id,
        token_program = %token_program_id,
        "SVM submitter initialized"
    );
    
    // 创建队列目录
    let queue_dir = &config.queue.path;
    std::fs::create_dir_all(queue_dir)?;
    info!(queue_path = %queue_dir.display(), "Queue directory initialized");
    
    // 持续处理队列中的事件
    loop {
        match process_queue(&config.queue.path, &signer, &rpc_client, &program_id, &usdc_mint, &token_program_id).await {
            Ok(processed) => {
                if processed > 0 {
                    info!(count = processed, "Processed events");
                }
            }
            Err(e) => {
                error!("Error processing queue: {}", e);
            }
        }
        
        // 等待后继续
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    }
}

/// 处理队列中的事件
async fn process_queue(
    queue_dir: &Path,
    signer: &Ed25519Signer,
    rpc_client: &RpcClient,
    program_id: &Pubkey,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
) -> Result<usize> {
    let mut processed = 0;
    
    // 读取队列目录中的所有事件文件
    let entries = std::fs::read_dir(queue_dir)?;
    
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            // 读取事件数据
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    match serde_json::from_str::<StakeEventData>(&content) {
                        Ok(event) => {
                            // 打印完整的事件信息
                            info!(
                                nonce = event.nonce,
                                amount = event.amount,
                                block_height = event.block_height,
                                sender = %event.sender,
                                receiver = %event.receiver_address,
                                source_contract = %event.source_contract,
                                target_contract = %event.target_contract,
                                source_chain_id = event.source_chain_id,
                                target_chain_id = event.target_chain_id,
                                "📥 Processing event from queue"
                            );
                            
                            // 处理事件
                            match submit_signature(signer, rpc_client, program_id, usdc_mint, token_program_id, &event).await {
                                Ok(tx_signature) => {
                                    info!(
                                        nonce = event.nonce,
                                        tx = tx_signature,
                                        "✅ Event processed successfully"
                                    );
                                    
                                    // 删除已处理的文件
                                    if let Err(e) = std::fs::remove_file(&path) {
                                        warn!("Failed to remove processed file: {}", e);
                                    }
                                    
                                    processed += 1;
                                }
                                Err(e) => {
                                    // 分析错误类型
                                    let error_str = format!("{}", e);
                                    let error_category = categorize_error(&error_str, &e);
                                    
                                    match error_category {
                                        ErrorCategory::NonRetryable => {
                                            // 不可重试错误：删除文件，避免无限重试
                                            warn!(
                                                nonce = event.nonce,
                                                error = %e,
                                                "Non-retryable error, removing event file"
                                            );
                                            if let Err(remove_err) = std::fs::remove_file(&path) {
                                                warn!("Failed to remove non-retryable event file: {}", remove_err);
                                            }
                                        }
                                        ErrorCategory::Retryable => {
                                            // 可重试错误：保留文件以便重试
                                            error!(
                                                nonce = event.nonce,
                                                error = %e,
                                                "Retryable error, keeping file for retry"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse event file {:?}: {}", path, e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read event file {:?}: {}", path, e);
                }
            }
        }
    }
    
    Ok(processed)
}

/// 提交签名到 SVM
async fn submit_signature(
    signer: &Ed25519Signer,
    rpc_client: &RpcClient,
    program_id: &Pubkey,
    usdc_mint: &Pubkey,
    token_program_id: &Pubkey,
    event: &StakeEventData,
) -> Result<String> {
    // 转换为精简格式
    let compact_event = event.to_compact()
        .map_err(|e| anyhow!("Failed to convert to compact format: {}", e))?;
    
    // 打印精简格式的详细信息
    info!(
        nonce = compact_event.nonce,
        amount = compact_event.amount,
        block_height = compact_event.block_height,
        sender_hex = %format!("0x{}", hex::encode(compact_event.sender)),
        receiver_pubkey = %bs58::encode(compact_event.receiver_pubkey).into_string(),
        "📦 Converted to compact format (76 bytes)"
    );
    
    // 生成签名（对精简格式的数据进行签名）
    let signature = signer.sign_compact_event(&compact_event)?;
    
    info!(
        signature_len = signature.len(),
        signature_hex = %hex::encode(&signature[..8]),
        "🔐 Generated Ed25519 signature"
    );
    
    // 推导 PDA 账户
    let (receiver_state, _) =
        Pubkey::find_program_address(&[b"receiver_state"], program_id);

    let (cross_chain_request, _) = Pubkey::find_program_address(
        &[b"cross_chain_request", &event.nonce.to_le_bytes()],
        program_id,
    );

    let (vault, _) = Pubkey::find_program_address(&[b"vault"], program_id);

    // 推导 token accounts（自动适配 SPL Token / Token-2022）
    let vault_token_account =
        spl_associated_token_account::get_associated_token_address_with_program_id(&vault, usdc_mint, token_program_id);

    // 创建 Ed25519 验证指令（使用精简格式）
    let ed25519_ix = create_ed25519_instruction_v2(signer, &compact_event, &signature)?;

    // 解析 receiver_address 为 Pubkey
    let receiver_pubkey = solana_sdk::pubkey::Pubkey::try_from(compact_event.receiver_pubkey)
        .map_err(|e| anyhow!("Invalid receiver pubkey: {}", e))?;
    
    let receiver_token_account =
        spl_associated_token_account::get_associated_token_address_with_program_id(&receiver_pubkey, usdc_mint, token_program_id);

    let create_ata_ix = if rpc_client.get_account(&receiver_token_account).is_err() {
        info!(
            receiver = %receiver_pubkey,
            ata = %receiver_token_account,
            "Receiver ATA not found, will create in transaction"
        );
        Some(
            spl_associated_token_account::instruction::create_associated_token_account_idempotent(
                &signer.keypair().pubkey(),
                &receiver_pubkey,
                usdc_mint,
                token_program_id,
            )
        )
    } else {
        None
    };

    // 创建 submit_signature 指令（使用精简格式）
    let submit_sig_ix = create_submit_signature_instruction(
        signer.keypair().pubkey(),
        program_id,
        &compact_event,
        &signature,
        receiver_state,
        cross_chain_request,
        vault,
        *usdc_mint,
        vault_token_account,
        receiver_token_account,
        receiver_pubkey,
        *token_program_id,
    )?;

    // 获取最新 blockhash
    let recent_blockhash = rpc_client
        .get_latest_blockhash()
        .map_err(|e| anyhow!("Failed to get latest blockhash: {}", e))?;

    let mut instructions = Vec::new();
    if let Some(ix) = create_ata_ix {
        instructions.push(ix);
    }
    instructions.push(ed25519_ix);
    instructions.push(submit_sig_ix);

    let mut transaction = Transaction::new_with_payer(
        &instructions,
        Some(&signer.keypair().pubkey()),
    );

    // 签名交易
    transaction.sign(&[signer.keypair()], recent_blockhash);

    // 输出交易详细信息用于调试
    info!(
        "🚀 Submitting transaction to SVM"
    );
    info!(
        nonce = compact_event.nonce,
        amount = compact_event.amount,
        block_height = compact_event.block_height,
        "  └─ Event data"
    );
    info!(
        sender = %format!("0x{}", hex::encode(compact_event.sender)),
        receiver = %receiver_pubkey,
        "  └─ Addresses"
    );
    info!(
        program_id = %program_id,
        relayer = %signer.keypair().pubkey(),
        "  └─ Program & Relayer"
    );
    info!(
        receiver_state = %receiver_state,
        cross_chain_request = %cross_chain_request,
        vault = %vault,
        "  └─ PDA Accounts"
    );
    info!(
        usdc_mint = %usdc_mint,
        vault_token_account = %vault_token_account,
        receiver_token_account = %receiver_token_account,
        "  └─ Token Accounts"
    );

    // 先模拟交易以获取详细错误信息
    let mut simulation_error: Option<(String, Option<Vec<String>>)> = None;
    match rpc_client.simulate_transaction(&transaction) {
        Ok(sim_result) => {
            if let Some(err) = sim_result.value.err {
                let error_msg = format!("Transaction simulation failed: {:?}\nLogs: {:?}", err, sim_result.value.logs);
                error!(
                    nonce = event.nonce,
                    error = ?err,
                    logs = ?sim_result.value.logs,
                    "Transaction simulation failed"
                );
                simulation_error = Some((error_msg, sim_result.value.logs));
            } else {
                info!(nonce = event.nonce, "Transaction simulation succeeded");
            }
        }
        Err(e) => {
            warn!(nonce = event.nonce, error = %e, "Failed to simulate transaction, proceeding anyway");
        }
    }

    // 如果模拟失败，返回错误（包含日志信息用于错误分类）
    if let Some((err_msg, _logs)) = simulation_error {
        return Err(anyhow!("{}", err_msg));
    }

    // 发送交易
    match rpc_client.send_and_confirm_transaction(&transaction) {
        Ok(sig) => {
            info!(nonce = event.nonce, tx = %sig, "Transaction confirmed");
            Ok(sig.to_string())
        }
        Err(e) => {
            let error_msg = format!("Failed to send transaction: {}", e);
            error!(nonce = event.nonce, error = %e, "Failed to send transaction");
            Err(anyhow!("{}", error_msg))
        }
    }
}

/// 创建 Ed25519 验证指令 (V2 - 使用精简格式)
/// 这个格式与 Solana web3.js 的 Ed25519Program.createInstructionWithPublicKey 兼容
fn create_ed25519_instruction_v2(
    signer: &Ed25519Signer,
    event: &CompactStakeEventData,
    signature: &[u8],
) -> Result<Instruction> {
    use borsh::BorshSerialize;
    
    // 序列化精简事件数据 - 这是要验证的原始消息
    let mut message = Vec::new();
    event.nonce.serialize(&mut message)?;
    event.amount.serialize(&mut message)?;
    event.block_height.serialize(&mut message)?;
    event.sender.serialize(&mut message)?;
    event.receiver_pubkey.serialize(&mut message)?;
    
    let pubkey_bytes = signer.keypair().pubkey().to_bytes();

    // 常量定义（与 Solana SDK 一致）
    const DATA_START: usize = 16;  // 2 (num_signatures + padding) + 14 (offsets struct)
    const PUBKEY_SIZE: usize = 32;
    const SIGNATURE_SIZE: usize = 64;

    let public_key_offset = DATA_START;
    let signature_offset = public_key_offset + PUBKEY_SIZE;
    let message_data_offset = signature_offset + SIGNATURE_SIZE;

    let mut instruction_data = Vec::with_capacity(
        DATA_START + PUBKEY_SIZE + SIGNATURE_SIZE + message.len()
    );

    // 1. num_signatures (u8) = 1
    instruction_data.push(1u8);
    // 2. padding (u8) = 0
    instruction_data.push(0u8);

    // 3. Ed25519SignatureOffsets 结构体 (14 bytes)
    // 使用 u16::MAX 表示数据在同一指令中
    instruction_data.extend_from_slice(&(signature_offset as u16).to_le_bytes());      // signature_offset
    instruction_data.extend_from_slice(&u16::MAX.to_le_bytes());                        // signature_instruction_index
    instruction_data.extend_from_slice(&(public_key_offset as u16).to_le_bytes());     // public_key_offset
    instruction_data.extend_from_slice(&u16::MAX.to_le_bytes());                        // public_key_instruction_index
    instruction_data.extend_from_slice(&(message_data_offset as u16).to_le_bytes());   // message_data_offset
    instruction_data.extend_from_slice(&(message.len() as u16).to_le_bytes());         // message_data_size
    instruction_data.extend_from_slice(&u16::MAX.to_le_bytes());                        // message_instruction_index

    // 4. 数据部分: pubkey -> signature -> message
    instruction_data.extend_from_slice(&pubkey_bytes);      // public key (32 bytes)
    instruction_data.extend_from_slice(signature);          // signature (64 bytes)
    instruction_data.extend_from_slice(&message);           // message (variable)

    // Ed25519Program ID
    let ed25519_program_id = Pubkey::new_from_array([
        3, 125, 70, 214, 124, 147, 251, 190,
        18, 249, 66, 143, 131, 141, 64, 255,
        5, 112, 116, 73, 39, 244, 138, 100,
        252, 202, 112, 68, 128, 0, 0, 0,
    ]);

    Ok(Instruction {
        program_id: ed25519_program_id,
        accounts: vec![],
        data: instruction_data,
    })
}

/// 创建 submit_signature 指令（使用精简格式）
#[allow(clippy::too_many_arguments)]
fn create_submit_signature_instruction(
    relayer_pubkey: Pubkey,
    program_id: &Pubkey,
    event: &CompactStakeEventData,
    signature: &[u8],
    receiver_state: Pubkey,
    cross_chain_request: Pubkey,
    vault: Pubkey,
    usdc_mint: Pubkey,
    vault_token_account: Pubkey,
    receiver_token_account: Pubkey,
    _receiver_pubkey: Pubkey,
    token_program_id: Pubkey,
) -> Result<Instruction> {
    use borsh::BorshSerialize;
    
    // Anchor 指令 discriminator (从 IDL 获取)
    // submit_signature 的 discriminator
    let discriminator: [u8; 8] = [205, 224, 80, 14, 239, 119, 52, 129];

    // 序列化参数
    // 函数签名: submit_signature(ctx, nonce: u64, event_data: StakeEventData, signature: Vec<u8>)
    let mut data = Vec::new();
    data.extend_from_slice(&discriminator);
    event.nonce.serialize(&mut data)?;          // 参数 1: nonce (u64)
    
    // 参数 2: event_data (CompactStakeEventData) - 按照 SVM 合约字段顺序
    event.nonce.serialize(&mut data)?;
    event.amount.serialize(&mut data)?;
    event.block_height.serialize(&mut data)?;
    event.sender.serialize(&mut data)?;
    event.receiver_pubkey.serialize(&mut data)?;
    
    signature.to_vec().serialize(&mut data)?;   // 参数 3: signature (Vec<u8>)

    // 构建账户列表
    let accounts = vec![
        AccountMeta::new(receiver_state, false),
        AccountMeta::new(cross_chain_request, false),
        AccountMeta::new(relayer_pubkey, true),
        AccountMeta::new(vault, false),
        AccountMeta::new_readonly(usdc_mint, false),
        AccountMeta::new(vault_token_account, false),
        AccountMeta::new(receiver_token_account, false),
        AccountMeta::new_readonly(sysvar::instructions::ID, false),
        AccountMeta::new_readonly(token_program_id, false),
        AccountMeta::new_readonly(system_program::ID, false),
    ];

    Ok(Instruction {
        program_id: *program_id,
        accounts,
        data,
    })
}

/// 分类错误类型：可重试 vs 不可重试
/// 
/// 核心逻辑：合约错误（Anchor错误码6000-6999）不可重试，其他错误可重试
fn categorize_error(error_str: &str, _error: &anyhow::Error) -> ErrorCategory {
    // 提取错误码：尝试从多种格式中提取
    // 1. Custom(6005)
    if let Some(start) = error_str.find("Custom(") {
        if let Some(end) = error_str[start..].find(')') {
            if let Ok(code) = error_str[start + 7..start + end].parse::<u32>() {
                if code >= 6000 && code < 7000 {
                    return ErrorCategory::NonRetryable;
                }
            }
        }
    }
    
    // 2. 0x1775 (十六进制)
    if let Some(start) = error_str.find("0x") {
        let hex_str = &error_str[start + 2..];
        let end = hex_str.chars().take_while(|c| c.is_ascii_hexdigit()).count();
        if end > 0 {
            if let Ok(code) = u32::from_str_radix(&hex_str[..end], 16) {
                if code >= 6000 && code < 7000 {
                    return ErrorCategory::NonRetryable;
                }
            }
        }
    }
    
    // 3. Error Number: 6005
    let error_lower = error_str.to_lowercase();
    if let Some(start) = error_lower.find("error number:") {
        let num_str = error_lower[start + 14..].split_whitespace().next().unwrap_or("");
        if let Ok(code) = num_str.parse::<u32>() {
            if code >= 6000 && code < 7000 {
                return ErrorCategory::NonRetryable;
            }
        }
    }
    
    // 4. 如果包含 "custom program error" 但没有提取到错误码，也认为是合约错误
    if error_lower.contains("custom program error") {
        return ErrorCategory::NonRetryable;
    }
    
    // 其他错误默认可重试
    ErrorCategory::Retryable
}

