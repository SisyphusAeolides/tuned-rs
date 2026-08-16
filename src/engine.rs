pub const MAX_TUNED_ARGUMENT_LENGTH: usize = 4096;

pub fn validate_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

pub fn profile_selection(selection: &str) -> Option<Vec<&str>> {
    let names = selection.split_whitespace().collect::<Vec<_>>();
    if names.is_empty() || names.iter().any(|name| !validate_profile_name(name)) {
        None
    } else {
        Some(names)
    }
}

pub fn normalize_profile_selection(selection: &str) -> Option<String> {
    profile_selection(selection).map(|names| names.join(" "))
}

pub fn validate_tuned_argument(value: &str) -> bool {
    value.len() <= MAX_TUNED_ARGUMENT_LENGTH
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    ' ' | '!'
                        | '@'
                        | '\''
                        | '+'
                        | '-'
                        | '.'
                        | ','
                        | '/'
                        | ':'
                        | ';'
                        | '_'
                        | '$'
                        | '&'
                        | '*'
                        | '('
                        | ')'
                        | '%'
                        | '<'
                        | '='
                        | '>'
                        | '?'
                        | '#'
                        | '['
                        | ']'
                        | '{'
                        | '|'
                        | '}'
                        | '^'
                        | '~'
                        | '"'
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_profile_names() {
        assert!(validate_profile_name("throughput-performance"));
        assert!(validate_profile_name("custom.v2"));
        assert!(!validate_profile_name("../etc/passwd"));
        assert!(!validate_profile_name("contains space"));
    }

    #[test]
    fn validates_and_normalizes_profile_selections() {
        assert_eq!(
            normalize_profile_selection("latency-performance   network-latency"),
            Some("latency-performance network-latency".to_string())
        );
        assert_eq!(normalize_profile_selection("../bad balanced"), None);
        assert_eq!(normalize_profile_selection("  "), None);
    }

    #[test]
    fn validates_exported_arguments_like_tuned() {
        assert!(validate_tuned_argument(""));
        assert!(validate_tuned_argument("cpu0,cpu1"));
        assert!(validate_tuned_argument("/run/tuned/events.sock"));
        assert!(validate_tuned_argument("devices=eth*;priority<10"));
        assert!(!validate_tuned_argument("line\nbreak"));
        assert!(!validate_tuned_argument(
            &"x".repeat(MAX_TUNED_ARGUMENT_LENGTH + 1)
        ));
    }
}
