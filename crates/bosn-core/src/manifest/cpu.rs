//! Typed task CPU requests, validated at the manifest boundary.
use super::ManifestError;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CpuRequest {
    Fixed(PositiveCpus),
    Auto,
}
impl Eq for CpuRequest {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositiveCpus(f64);
impl Eq for PositiveCpus {}
impl PositiveCpus {
    pub fn get(self) -> f64 {
        self.0
    }
}
impl std::fmt::Display for CpuRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Fixed(n) => n.0.fmt(f),
        }
    }
}
pub(super) fn parse(
    value: Option<&toml::Value>,
    task: &str,
) -> Result<Option<CpuRequest>, ManifestError> {
    let Some(value) = value else { return Ok(None) };
    let number = match value {
        toml::Value::String(s) if s == "auto" => return Ok(Some(CpuRequest::Auto)),
        toml::Value::Integer(n) => Some(*n as f64),
        toml::Value::Float(n) => Some(*n),
        _ => None,
    };
    match number {
        Some(n) if n.is_finite() && n > 0.0 => Ok(Some(CpuRequest::Fixed(PositiveCpus(n)))),
        _ => Err(ManifestError(format!(
            "[task.{task}] `cpus` must be a positive finite number or \"auto\""
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_only_positive_numbers_or_auto() {
        for text in ["1", "0.25", "\"auto\""] {
            let table: toml::Table = format!("cpus = {text}").parse().unwrap();
            assert!(parse(table.get("cpus"), "test").unwrap().is_some());
        }
        for text in ["0", "-1", "nan", "inf", "\"fast\"", "true", "[]"] {
            let table: toml::Table = format!("cpus = {text}").parse().unwrap();
            assert!(
                parse(table.get("cpus"), "test")
                    .unwrap_err()
                    .0
                    .contains("task.test")
            );
        }
        assert_eq!(parse(None, "test").unwrap(), None);
    }
}
