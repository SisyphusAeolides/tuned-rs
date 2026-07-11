use anyhow::{bail, Result};
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentOp {
    Set,
    Greater,
    GreaterEqual,
    Less,
    LessEqual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub op: AssignmentOp,
    pub raw: String,
    pub target: String,
}

pub fn parse_assignment(raw: &str) -> Assignment {
    let raw = raw.trim();
    for (prefix, op) in [
        (">=", AssignmentOp::GreaterEqual),
        ("=>", AssignmentOp::GreaterEqual),
        ("<=", AssignmentOp::LessEqual),
        ("=<", AssignmentOp::LessEqual),
        (">", AssignmentOp::Greater),
        ("<", AssignmentOp::Less),
    ] {
        if let Some(target) = raw.strip_prefix(prefix) {
            return Assignment {
                op,
                raw: raw.to_string(),
                target: target.trim().to_string(),
            };
        }
    }

    Assignment {
        op: AssignmentOp::Set,
        raw: raw.to_string(),
        target: raw.to_string(),
    }
}

pub fn resolve_numeric_assignment(assignment: &Assignment, current: &str) -> Result<Option<String>> {
    let current = current.trim();
    if assignment.op == AssignmentOp::Set {
        return Ok(Some(assignment.target.clone()));
    }

    let target = assignment.target.parse::<i64>();
    let current_value = current.parse::<i64>();
    if let (Ok(target), Ok(current_value)) = (target, current_value) {
        let apply = match assignment.op {
            AssignmentOp::Greater | AssignmentOp::GreaterEqual => target > current_value,
            AssignmentOp::Less | AssignmentOp::LessEqual => target < current_value,
            AssignmentOp::Set => true,
        };
        return Ok(if apply {
            Some(assignment.target.clone())
        } else {
            None
        });
    }

    warn!(
        "Cannot compare assignment '{}' with current value '{}'; using direct assignment",
        assignment.raw, current
    );
    Ok(Some(assignment.target.clone()))
}

pub fn resolve_choice(raw: &str, is_valid: impl Fn(&str) -> bool) -> Option<String> {
    for candidate in raw.split('|').map(str::trim).filter(|part| !part.is_empty()) {
        if is_valid(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

pub fn read_trimmed(path: &std::path::Path) -> Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let trimmed = content.trim();
    if trimmed.contains('\n') {
        bail!("Multi-line values are unsupported for {}", path.display());
    }
    Ok(trimmed.to_string())
}

use anyhow::Context;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_char_operators() {
        let assignment = parse_assignment("=>4096");
        assert_eq!(assignment.op, AssignmentOp::GreaterEqual);
        assert_eq!(assignment.target, "4096");
    }

    #[test]
    fn resolves_greater_equal_only_when_needed() {
        let assignment = parse_assignment("=>4096");
        assert_eq!(
            resolve_numeric_assignment(&assignment, "8192").unwrap(),
            None
        );
        assert_eq!(
            resolve_numeric_assignment(&assignment, "2048").unwrap(),
            Some("4096".to_string())
        );
    }

    #[test]
    fn resolves_less_than_operator() {
        let assignment = parse_assignment("<100");
        assert_eq!(
            resolve_numeric_assignment(&assignment, "120").unwrap(),
            Some("100".to_string())
        );
        assert_eq!(
            resolve_numeric_assignment(&assignment, "80").unwrap(),
            None
        );
    }
}
