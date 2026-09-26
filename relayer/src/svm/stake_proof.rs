//! SVM `Staked` 事件的链上状态佐证（#1133 / BUG-CHAIN-009）
//!
//! 链上 `confirm_event` 的所有校验字段都取自中继器上报的 event_data 本身——
//! **中继器就是预言机**。日志归属（`log_attribution`）挡住了"别的程序打的日志"，
//! 这里再加一道与日志完全独立的证据：
//!
//! 桥合约每笔 `stake` 都 `init` 一个 `StakeRecord` PDA
//! （seeds = `["stake_record", nonce.to_le_bytes()]`），写入
//! `owner = user`、`amount = actual_amount`（= 事件的 `raw_amount`），
//! hub 形态还写 `target_chain_id`。这个账户只能由桥程序创建和写入——
//! 攻击者既无法在该地址建账户，也无法让它归属桥程序。
//!
//! 所以对每个待中继的 `Staked`，要求：
//! - PDA 存在，且 `owner` 程序 == 桥程序；
//! - Anchor 账户判别符 == `StakeRecord`；
//! - `owner == event.sender`、`amount == event.raw_amount`
//!   （hub：另要求 `target_chain_id == event.target_chain_id`）；
//! - 未退款、未发起退款（发起退款意味着运营方已决定在源链退回，
//!   再去目标链解锁就是双花）。
//!
//! 任何一条不满足 → 不中继（由调用方转 DLQ 人工核查）。

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;

use crate::types::{BridgeEventData, SvmProgramKind};

/// `StakeRecord` PDA 地址（hub / leaf 相同 seeds）。
pub fn stake_record_pda(program_id: &Pubkey, nonce: u64) -> Pubkey {
    Pubkey::find_program_address(&[b"stake_record", nonce.to_le_bytes().as_ref()], program_id).0
}

fn stake_record_account_disc() -> [u8; 8] {
    let h = Sha256::digest(b"account:StakeRecord");
    let mut out = [0u8; 8];
    out.copy_from_slice(&h[..8]);
    out
}

/// 链上 `StakeRecord` 的解码结果（hub 多一个 `target_chain_id`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakeRecord {
    pub owner: [u8; 32],
    pub amount: u64,
    pub target_chain_id: Option<u64>,
    pub refunded: bool,
    pub refund_initiated_at: u64,
}

fn read_u64(data: &[u8], at: usize) -> Result<u64> {
    let bytes: [u8; 8] = data
        .get(at..at + 8)
        .context("StakeRecord 数据过短")?
        .try_into()
        .expect("slice len 8");
    Ok(u64::from_le_bytes(bytes))
}

/// 按程序形态解码 `StakeRecord` 账户数据（含 8B Anchor 判别符）。
///
/// hub：disc | owner 32 | amount 8 | target_chain_id 8 | refunded 1 | refund_initiated_at 8
/// leaf：disc | owner 32 | amount 8 | refunded 1 | refund_initiated_at 8
pub fn parse_stake_record(kind: SvmProgramKind, data: &[u8]) -> Result<StakeRecord> {
    if data.len() < 8 || data[..8] != stake_record_account_disc() {
        bail!("账户判别符不是 StakeRecord");
    }
    let mut owner = [0u8; 32];
    owner.copy_from_slice(data.get(8..40).context("StakeRecord 数据过短")?);
    let amount = read_u64(data, 40)?;
    let (target_chain_id, flag_at) = match kind {
        SvmProgramKind::Hub => (Some(read_u64(data, 48)?), 56),
        SvmProgramKind::Leaf => (None, 48),
    };
    let refunded = match data.get(flag_at).context("StakeRecord 数据过短")? {
        0 => false,
        1 => true,
        other => bail!("StakeRecord.refunded 非法取值 {other}"),
    };
    let refund_initiated_at = read_u64(data, flag_at + 1)?;
    Ok(StakeRecord {
        owner,
        amount,
        target_chain_id,
        refunded,
        refund_initiated_at,
    })
}

/// 纯函数：账户（归属程序 + 数据）是否佐证了这条 `Staked`。
pub fn check_stake_record(
    kind: SvmProgramKind,
    program_id: &Pubkey,
    account_owner: &Pubkey,
    data: &[u8],
    ev: &BridgeEventData,
) -> Result<()> {
    if account_owner != program_id {
        bail!("StakeRecord 归属程序 {account_owner} ≠ 桥程序 {program_id}");
    }
    let rec = parse_stake_record(kind, data)?;
    if rec.owner != ev.sender {
        bail!(
            "StakeRecord.owner {} ≠ 事件 sender {}",
            Pubkey::new_from_array(rec.owner),
            Pubkey::new_from_array(ev.sender)
        );
    }
    if rec.amount != ev.raw_amount {
        bail!(
            "StakeRecord.amount {} ≠ 事件 raw_amount {}",
            rec.amount,
            ev.raw_amount
        );
    }
    if ev.amount > ev.raw_amount {
        bail!("事件 amount {} > raw_amount {}", ev.amount, ev.raw_amount);
    }
    if let Some(target) = rec.target_chain_id {
        if target != ev.target_chain_id {
            bail!(
                "StakeRecord.target_chain_id {target} ≠ 事件 target_chain_id {}",
                ev.target_chain_id
            );
        }
    }
    if rec.refunded || rec.refund_initiated_at != 0 {
        bail!(
            "StakeRecord 已退款/已发起退款（refunded={}, refund_initiated_at={}），不得再中继",
            rec.refunded,
            rec.refund_initiated_at
        );
    }
    Ok(())
}

/// 读 finalized 的 `StakeRecord` 并核对事件。
///
/// PDA 不存在 → Err：交易本身已 finalized，账户必然已在；读不到只可能是
/// RPC 滞后（重试可恢复）或事件是伪造的（重试到 DLQ 由人核查）。
pub async fn verify_staked_against_chain(
    rpc: &RpcClient,
    kind: SvmProgramKind,
    program_id: &Pubkey,
    ev: &BridgeEventData,
) -> Result<()> {
    let pda = stake_record_pda(program_id, ev.nonce);
    let account = rpc
        .get_account_with_commitment(&pda, CommitmentConfig::finalized())
        .await
        .with_context(|| format!("读取 StakeRecord {pda} 失败"))?
        .value
        .with_context(|| format!("StakeRecord {pda}（nonce={}）不存在", ev.nonce))?;
    check_stake_record(kind, program_id, &account.owner, &account.data, ev)
        .with_context(|| format!("StakeRecord {pda}（nonce={}）与事件不符", ev.nonce))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program() -> Pubkey {
        Pubkey::new_from_array([0xB1; 32])
    }

    fn event() -> BridgeEventData {
        BridgeEventData {
            source_contract: program().to_bytes(),
            target_contract: [0x02; 32],
            source_chain_id: 91026,
            target_chain_id: 11155111,
            block_height: 100,
            raw_amount: 5_000_000,
            amount: 4_800_000,
            sender: [0x33; 32],
            receiver: [0x44; 32],
            nonce: 9,
        }
    }

    fn record_bytes(kind: SvmProgramKind, owner: [u8; 32], amount: u64, target: u64) -> Vec<u8> {
        let mut d = stake_record_account_disc().to_vec();
        d.extend_from_slice(&owner);
        d.extend_from_slice(&amount.to_le_bytes());
        if kind == SvmProgramKind::Hub {
            d.extend_from_slice(&target.to_le_bytes());
        }
        d.push(0);
        d.extend_from_slice(&0u64.to_le_bytes());
        d
    }

    #[test]
    fn hub_record_matching_event_passes() {
        let ev = event();
        let d = record_bytes(
            SvmProgramKind::Hub,
            ev.sender,
            ev.raw_amount,
            ev.target_chain_id,
        );
        assert_eq!(d.len(), 65, "hub StakeRecord::LEN");
        check_stake_record(SvmProgramKind::Hub, &program(), &program(), &d, &ev).unwrap();
    }

    #[test]
    fn leaf_record_matching_event_passes() {
        let ev = event();
        let d = record_bytes(SvmProgramKind::Leaf, ev.sender, ev.raw_amount, 0);
        assert_eq!(d.len(), 57, "leaf StakeRecord::LEN");
        check_stake_record(SvmProgramKind::Leaf, &program(), &program(), &d, &ev).unwrap();
    }

    #[test]
    fn account_not_owned_by_bridge_is_rejected() {
        let ev = event();
        let d = record_bytes(
            SvmProgramKind::Hub,
            ev.sender,
            ev.raw_amount,
            ev.target_chain_id,
        );
        let other = Pubkey::new_from_array([0xEE; 32]);
        assert!(check_stake_record(SvmProgramKind::Hub, &program(), &other, &d, &ev).is_err());
    }

    #[test]
    fn inflated_amount_is_rejected() {
        let mut ev = event();
        let d = record_bytes(
            SvmProgramKind::Hub,
            ev.sender,
            ev.raw_amount,
            ev.target_chain_id,
        );
        ev.raw_amount *= 1000;
        assert!(check_stake_record(SvmProgramKind::Hub, &program(), &program(), &d, &ev).is_err());
    }

    #[test]
    fn net_amount_above_raw_is_rejected() {
        let mut ev = event();
        let d = record_bytes(
            SvmProgramKind::Hub,
            ev.sender,
            ev.raw_amount,
            ev.target_chain_id,
        );
        ev.amount = ev.raw_amount + 1;
        assert!(check_stake_record(SvmProgramKind::Hub, &program(), &program(), &d, &ev).is_err());
    }

    #[test]
    fn different_sender_is_rejected() {
        let ev = event();
        let d = record_bytes(
            SvmProgramKind::Hub,
            [0x99; 32],
            ev.raw_amount,
            ev.target_chain_id,
        );
        assert!(check_stake_record(SvmProgramKind::Hub, &program(), &program(), &d, &ev).is_err());
    }

    #[test]
    fn redirected_target_chain_is_rejected_on_hub() {
        let ev = event();
        let d = record_bytes(SvmProgramKind::Hub, ev.sender, ev.raw_amount, 84532);
        assert!(check_stake_record(SvmProgramKind::Hub, &program(), &program(), &d, &ev).is_err());
    }

    #[test]
    fn refunded_or_refund_pending_is_rejected() {
        let ev = event();
        let mut d = record_bytes(SvmProgramKind::Leaf, ev.sender, ev.raw_amount, 0);
        d[48] = 1; // refunded
        assert!(check_stake_record(SvmProgramKind::Leaf, &program(), &program(), &d, &ev).is_err());

        let mut d = record_bytes(SvmProgramKind::Leaf, ev.sender, ev.raw_amount, 0);
        d[49..57].copy_from_slice(&1_700_000_000u64.to_le_bytes());
        assert!(check_stake_record(SvmProgramKind::Leaf, &program(), &program(), &d, &ev).is_err());
    }

    #[test]
    fn wrong_discriminator_is_rejected() {
        let ev = event();
        let mut d = record_bytes(
            SvmProgramKind::Hub,
            ev.sender,
            ev.raw_amount,
            ev.target_chain_id,
        );
        d[0] ^= 0xFF;
        assert!(check_stake_record(SvmProgramKind::Hub, &program(), &program(), &d, &ev).is_err());
    }
}
