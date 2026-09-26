//! SVM 交易日志归属（#1133 / BUG-CHAIN-009）
//!
//! `meta.log_messages` 是**整笔交易**所有程序的日志拼在一起的扁平列表。
//! `Program data: <base64>` 行本身不带"是谁打的"——同一笔交易里任何程序都能
//! 用 `sol_log_data` 打出一行布局完全相同的 `Staked`。只按前缀 + 判别符匹配，
//! 等于把**任意第三方程序**的日志当成桥合约事件（EVM 侧 `eth_getLogs` 按
//! 合约地址过滤、节点保证归属，所以只有 SVM 侧有这个洞）。
//!
//! 归属只能从运行时自己打的调用栈行还原：
//!
//! ```text
//! Program <id> invoke [depth]     ← 运行时打，depth 从 1 起
//! Program <id> success            ← 运行时打
//! Program <id> failed: <err>      ← 运行时打
//! Program data: <base64>          ← 归属于此刻栈顶的程序
//! ```
//!
//! 程序自己能写的只有 `Program log: ` / `Program data: ` / `Program return: `
//! / `Program consumption: ` 这几种前缀的行（`sol_log*` 系列都由运行时加前缀），
//! 伪造不出 `Program <id> invoke [n]`；单条日志里带换行也仍是**一条**，
//! 不会被拆成两行。所以栈是可信的，`Program data:` 行的发出者由栈顶唯一确定。
//!
//! 失败即关闭（fail closed）：
//! - 栈结构对不上（depth 跳号 / success 与栈顶不符 / 空栈上出 data 行）→
//!   `Malformed`，整笔交易不采信；
//! - 运行时截断了日志（`Log truncated`，单笔 10KB 上限）→ `Truncated`，
//!   截断之后的事件已经丢了，只能交人工核查，不能假装"没有事件"。

use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

/// 运行时截断日志时追加的固定行。
const LOG_TRUNCATED: &str = "Log truncated";

/// 一笔交易里，被归属到目标程序的 `Program data:` 载荷（base64 原文），
/// 以及其它程序打出的 `Program data:` 行（供调用方识别"伪装成桥事件"的行并告警）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct AttributedProgramData<'a> {
    /// 栈顶 == 目标程序时打出的 data 行（去掉前缀、已 trim）
    pub own: Vec<&'a str>,
    /// 其它程序打出的 data 行：(发出者, base64 原文)
    pub foreign: Vec<(Pubkey, &'a str)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogAttributionError {
    /// 日志被运行时截断，截断之后的行不可见
    Truncated,
    /// 调用栈对不上，无法可信归属
    Malformed(String),
}

impl LogAttributionError {
    /// 稳定的短原因码，用于日志 / 与 `SigLogsOutcome::Unfetchable` 对接
    pub fn reason(&self) -> &'static str {
        match self {
            LogAttributionError::Truncated => "log-truncated",
            LogAttributionError::Malformed(_) => "log-stack-malformed",
        }
    }
}

impl std::fmt::Display for LogAttributionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogAttributionError::Truncated => write!(f, "交易日志被运行时截断（Log truncated）"),
            LogAttributionError::Malformed(why) => write!(f, "交易日志调用栈不自洽: {why}"),
        }
    }
}

impl std::error::Error for LogAttributionError {}

/// 运行时打出的调用栈行
enum StackLine {
    Invoke {
        program: Pubkey,
        depth: usize,
    },
    /// success 或 failed
    Exit {
        program: Pubkey,
    },
}

/// 严格解析运行时的 invoke / success / failed 行；其它行一律返回 None。
fn parse_stack_line(line: &str) -> Option<StackLine> {
    let rest = line.strip_prefix("Program ")?;
    let (id, tail) = rest.split_once(' ')?;
    // `Program log:` / `Program data:` / `Program return:` 等的第二个 token
    // 以冒号结尾，不是 base58 pubkey，这里自然被排除。
    let program = Pubkey::from_str(id).ok()?;
    if tail == "success" || tail.starts_with("failed") {
        return Some(StackLine::Exit { program });
    }
    let depth = tail
        .strip_prefix("invoke [")?
        .strip_suffix(']')?
        .parse::<usize>()
        .ok()?;
    Some(StackLine::Invoke { program, depth })
}

/// 把交易日志里的 `Program data:` 行按调用栈归属到发出程序，
/// 只把 `program_id` **自己**打出的行放进 `own`。
pub fn attribute_program_data<'a>(
    logs: &'a [String],
    program_id: &Pubkey,
) -> Result<AttributedProgramData<'a>, LogAttributionError> {
    let mut stack: Vec<Pubkey> = Vec::new();
    let mut out = AttributedProgramData::default();

    for (i, line) in logs.iter().enumerate() {
        if line == LOG_TRUNCATED {
            return Err(LogAttributionError::Truncated);
        }
        if let Some(data) = line.strip_prefix("Program data: ") {
            let Some(emitter) = stack.last() else {
                return Err(LogAttributionError::Malformed(format!(
                    "第 {i} 行 Program data 出现在任何 invoke 之外"
                )));
            };
            if emitter == program_id {
                out.own.push(data.trim());
            } else {
                out.foreign.push((*emitter, data.trim()));
            }
            continue;
        }
        match parse_stack_line(line) {
            Some(StackLine::Invoke { program, depth }) => {
                if depth != stack.len() + 1 {
                    return Err(LogAttributionError::Malformed(format!(
                        "第 {i} 行 invoke 深度 {depth}，期望 {}",
                        stack.len() + 1
                    )));
                }
                stack.push(program);
            }
            Some(StackLine::Exit { program }) => match stack.pop() {
                Some(top) if top == program => {}
                Some(top) => {
                    return Err(LogAttributionError::Malformed(format!(
                        "第 {i} 行 {program} 退出，但栈顶是 {top}"
                    )));
                }
                None => {
                    return Err(LogAttributionError::Malformed(format!(
                        "第 {i} 行 {program} 退出时栈为空"
                    )));
                }
            },
            None => {}
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge() -> Pubkey {
        Pubkey::new_from_array([7u8; 32])
    }
    fn attacker() -> Pubkey {
        Pubkey::new_from_array([9u8; 32])
    }
    fn token() -> Pubkey {
        Pubkey::new_from_array([3u8; 32])
    }

    fn l(s: impl Into<String>) -> String {
        s.into()
    }

    /// 真实 stake 交易的形态：桥合约顶层 → CPI token 程序 → 回到桥合约 emit。
    #[test]
    fn own_data_after_cpi_returns_is_attributed_to_bridge() {
        let (b, t) = (bridge(), token());
        let logs = vec![
            l(format!("Program {b} invoke [1]")),
            l("Program log: Instruction: Stake"),
            l(format!("Program {t} invoke [2]")),
            l("Program log: Instruction: TransferChecked"),
            l(format!("Program {t} consumed 6200 of 180000 compute units")),
            l(format!("Program {t} success")),
            l("Program data: AAAA"),
            l(format!(
                "Program {b} consumed 40000 of 200000 compute units"
            )),
            l(format!("Program {b} success")),
        ];
        let got = attribute_program_data(&logs, &b).unwrap();
        assert_eq!(got.own, vec!["AAAA"]);
        assert!(got.foreign.is_empty());
    }

    /// #1133 攻击形态：攻击者程序与桥合约同处一笔交易，自己打一行 Staked 布局的 data。
    #[test]
    fn data_from_sibling_program_is_foreign() {
        let (b, a) = (bridge(), attacker());
        let logs = vec![
            l(format!("Program {b} invoke [1]")),
            l("Program log: Instruction: Configure"),
            l(format!("Program {b} success")),
            l(format!("Program {a} invoke [1]")),
            l("Program data: FORGED"),
            l(format!("Program {a} success")),
        ];
        let got = attribute_program_data(&logs, &b).unwrap();
        assert!(got.own.is_empty());
        assert_eq!(got.foreign, vec![(a, "FORGED")]);
    }

    /// 攻击者程序 CPI 调桥合约之后、在自己的帧里伪造 —— 仍归攻击者。
    #[test]
    fn data_emitted_by_caller_after_cpi_into_bridge_is_foreign() {
        let (b, a) = (bridge(), attacker());
        let logs = vec![
            l(format!("Program {a} invoke [1]")),
            l(format!("Program {b} invoke [2]")),
            l("Program data: REAL"),
            l(format!("Program {b} success")),
            l("Program data: FORGED"),
            l(format!("Program {a} success")),
        ];
        let got = attribute_program_data(&logs, &b).unwrap();
        assert_eq!(got.own, vec!["REAL"]);
        assert_eq!(got.foreign, vec![(a, "FORGED")]);
    }

    /// 程序自己 msg! 出一段看起来像运行时行的文本：带 `Program log: ` 前缀，不影响栈。
    #[test]
    fn msg_imitating_runtime_lines_does_not_move_the_stack() {
        let (b, a) = (bridge(), attacker());
        let logs = vec![
            l(format!("Program {a} invoke [1]")),
            l(format!("Program log: Program {b} invoke [2]")),
            l("Program data: FORGED"),
            l(format!("Program {a} success")),
        ];
        let got = attribute_program_data(&logs, &b).unwrap();
        assert!(got.own.is_empty());
        assert_eq!(got.foreign.len(), 1);
    }

    #[test]
    fn truncated_logs_fail_closed() {
        let b = bridge();
        let logs = vec![
            l(format!("Program {b} invoke [1]")),
            l("Program data: AAAA"),
            l("Log truncated"),
        ];
        assert_eq!(
            attribute_program_data(&logs, &b),
            Err(LogAttributionError::Truncated)
        );
    }

    #[test]
    fn depth_skip_is_malformed() {
        let b = bridge();
        let logs = vec![
            l(format!("Program {b} invoke [2]")),
            l("Program data: AAAA"),
        ];
        assert!(matches!(
            attribute_program_data(&logs, &b),
            Err(LogAttributionError::Malformed(_))
        ));
    }

    #[test]
    fn exit_mismatch_is_malformed() {
        let (b, a) = (bridge(), attacker());
        let logs = vec![
            l(format!("Program {a} invoke [1]")),
            l(format!("Program {b} success")),
        ];
        assert!(matches!(
            attribute_program_data(&logs, &b),
            Err(LogAttributionError::Malformed(_))
        ));
    }

    #[test]
    fn data_outside_any_frame_is_malformed() {
        let logs = vec![l("Program data: AAAA")];
        assert!(matches!(
            attribute_program_data(&logs, &bridge()),
            Err(LogAttributionError::Malformed(_))
        ));
    }

    /// `failed: ...` 也是退栈（只可能出现在已被上游过滤的失败交易里，但解析必须正确）。
    #[test]
    fn failed_line_pops_the_frame() {
        let (b, a) = (bridge(), attacker());
        let logs = vec![
            l(format!("Program {a} invoke [1]")),
            l(format!("Program {a} failed: custom program error: 0x1")),
            l(format!("Program {b} invoke [1]")),
            l("Program data: REAL"),
            l(format!("Program {b} success")),
        ];
        let got = attribute_program_data(&logs, &b).unwrap();
        assert_eq!(got.own, vec!["REAL"]);
    }
}
