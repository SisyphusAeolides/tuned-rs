pub fn validate_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_profile_names() {
        assert!(validate_profile_name("throughput-performance"));
        assert!(!validate_profile_name("../etc/passwd"));
    }
}
