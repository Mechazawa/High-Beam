#[must_use]
pub fn placeholder() -> &'static str {
    "high-beam"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_returns_crate_name() {
        assert_eq!(placeholder(), "high-beam");
    }
}
