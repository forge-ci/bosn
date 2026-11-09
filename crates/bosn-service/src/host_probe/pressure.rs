//! Linux pressure-stall averages, without process-level information.

pub(crate) fn avg10(text: &str, category: &str) -> Option<f64> {
    let line = text
        .lines()
        .find(|line| line.split_whitespace().next() == Some(category))?;
    let value = line
        .split_whitespace()
        .skip(1)
        .find_map(|field| field.strip_prefix("avg10="))?
        .parse::<f64>()
        .ok()?;
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_categories_are_independent_and_invalid_values_are_unknown() {
        let text = "some avg10=2.50 avg60=1 total=123\nfull avg10=0.10 avg60=0 total=3\n";
        assert_eq!(avg10(text, "some"), Some(2.5));
        assert_eq!(avg10(text, "full"), Some(0.1));
        assert_eq!(avg10("some avg10=NaN", "some"), None);
        assert_eq!(avg10("some avg10=101", "some"), None);
        assert_eq!(avg10("some total=3", "some"), None);
    }
}
