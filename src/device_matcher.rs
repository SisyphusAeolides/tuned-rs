use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRules {
    positive: Vec<String>,
    negative: Vec<String>,
}

impl DeviceRules {
    pub fn parse(raw: &str) -> Self {
        let mut positive = Vec::new();
        let mut negative = Vec::new();

        for rule in raw
            .split_whitespace()
            .flat_map(|field| field.split(','))
            .map(str::trim)
            .filter(|rule| !rule.is_empty())
        {
            if let Some(rule) = rule.strip_prefix('!') {
                if !rule.is_empty() {
                    negative.push(rule.to_string());
                }
            } else {
                positive.push(rule.to_string());
            }
        }

        if positive.is_empty() {
            positive.push("*".to_string());
        }

        Self { positive, negative }
    }

    pub fn matches(&self, device_name: &str) -> bool {
        self.positive
            .iter()
            .any(|rule| glob_matches(rule, device_name))
            && !self
                .negative
                .iter()
                .any(|rule| glob_matches(rule, device_name))
    }
}

pub fn matches(raw: &str, device_name: &str) -> bool {
    DeviceRules::parse(raw).matches(device_name)
}

pub fn filter_names<I>(raw: &str, names: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let rules = DeviceRules::parse(raw);
    let mut selected = names
        .into_iter()
        .filter(|name| rules.matches(name))
        .collect::<Vec<_>>();
    selected.sort_unstable();
    selected.dedup();
    selected
}

pub(crate) fn glob_matches(pattern: &str, text: &str) -> bool {
    fn matches_from(
        pattern: &[u8],
        text: &[u8],
        pattern_index: usize,
        text_index: usize,
        memo: &mut HashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(cached) = memo.get(&(pattern_index, text_index)) {
            return *cached;
        }

        let result = if pattern_index == pattern.len() {
            text_index == text.len()
        } else {
            match pattern[pattern_index] {
                b'*' => {
                    let mut next_pattern = pattern_index + 1;
                    while next_pattern < pattern.len() && pattern[next_pattern] == b'*' {
                        next_pattern += 1;
                    }
                    if next_pattern == pattern.len() {
                        true
                    } else {
                        (text_index..=text.len()).any(|next_text| {
                            matches_from(pattern, text, next_pattern, next_text, memo)
                        })
                    }
                }
                b'?' => {
                    text_index < text.len()
                        && matches_from(pattern, text, pattern_index + 1, text_index + 1, memo)
                }
                b'[' if text_index < text.len() => {
                    match character_class(pattern, pattern_index, text[text_index]) {
                        Some((matched, next_pattern)) => {
                            matched
                                && matches_from(pattern, text, next_pattern, text_index + 1, memo)
                        }
                        None => {
                            text[text_index] == b'['
                                && matches_from(
                                    pattern,
                                    text,
                                    pattern_index + 1,
                                    text_index + 1,
                                    memo,
                                )
                        }
                    }
                }
                b'\\' if pattern_index + 1 < pattern.len() => {
                    text_index < text.len()
                        && text[text_index] == pattern[pattern_index + 1]
                        && matches_from(pattern, text, pattern_index + 2, text_index + 1, memo)
                }
                byte => {
                    text_index < text.len()
                        && text[text_index] == byte
                        && matches_from(pattern, text, pattern_index + 1, text_index + 1, memo)
                }
            }
        };

        memo.insert((pattern_index, text_index), result);
        result
    }

    matches_from(
        pattern.as_bytes(),
        text.as_bytes(),
        0,
        0,
        &mut HashMap::new(),
    )
}

fn character_class(pattern: &[u8], opening: usize, value: u8) -> Option<(bool, usize)> {
    let closing = pattern[opening + 1..]
        .iter()
        .position(|byte| *byte == b']')?
        + opening
        + 1;
    if closing == opening + 1 {
        return None;
    }

    let mut cursor = opening + 1;
    let negated = matches!(pattern[cursor], b'!' | b'^');
    if negated {
        cursor += 1;
    }

    let mut matched = false;
    while cursor < closing {
        if cursor + 2 < closing && pattern[cursor + 1] == b'-' {
            let start = pattern[cursor];
            let end = pattern[cursor + 2];
            if start <= value && value <= end {
                matched = true;
            }
            cursor += 3;
        } else {
            if pattern[cursor] == value {
                matched = true;
            }
            cursor += 1;
        }
    }

    Some((if negated { !matched } else { matched }, closing + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_and_negative_rules_match_upstream_order_independently() {
        let rules = DeviceRules::parse("sd* !sda");
        assert!(!rules.matches("sda"));
        assert!(rules.matches("sdb"));
        assert!(!rules.matches("nvme0n1"));
    }

    #[test]
    fn negative_only_rules_have_an_implicit_match_all_rule() {
        let rules = DeviceRules::parse("!loop* !ram*");
        assert!(rules.matches("nvme0n1"));
        assert!(!rules.matches("loop0"));
    }

    #[test]
    fn comma_space_classes_and_escapes_are_supported() {
        let rules = DeviceRules::parse(r"nvme[0-9]n?, sd\*literal");
        assert!(rules.matches("nvme0n1"));
        assert!(rules.matches("sd*literal"));
        assert!(!rules.matches("nvme10n1"));
    }

    #[test]
    fn filtering_is_sorted_and_duplicate_free() {
        let selected = filter_names(
            "sd* !sda",
            vec![
                "sdc".to_string(),
                "sdb".to_string(),
                "sdb".to_string(),
                "sda".to_string(),
            ],
        );
        assert_eq!(selected, ["sdb", "sdc"]);
    }
}
