pub use crate::tuning::cpu::{is_allowed_epp, is_allowed_governor};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_allowlisted_governors() {
        assert!(!is_allowed_governor("totally-made-up"));
        assert!(is_allowed_governor("schedutil"));
    }
}
