//! Finite native protection, with room for admitted work and stopped recovery.
use std::time::Duration;

const LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
const RECOVERY_MARGIN: Duration = Duration::from_secs(60 * 60);

pub(crate) struct Window {
    pub created: i64,
    pub expires: i64,
    pub admit_until: i64,
}

impl Window {
    /// The engine's immutable creation epoch also fixes every retry's pin.
    /// Never extend an existing owner or admit a future/expired epoch.
    pub fn new(created: f64, now: f64) -> Result<Self, String> {
        if !created.is_finite()
            || !now.is_finite()
            || created < 0.0
            || now < created
            || created > ((1_u64 << 53) - LIFETIME.as_secs()) as f64
        {
            return Err("native recovery epoch is invalid".into());
        }
        let created = created.floor() as i64;
        let expires = created + LIFETIME.as_secs() as i64;
        let admit_until = expires
            - super::wire::MAX_RUN_TIMEOUT.as_secs() as i64
            - RECOVERY_MARGIN.as_secs() as i64;
        if now >= admit_until as f64 {
            return Err("native recovery admission window has closed; replan the engine".into());
        }
        Ok(Self {
            created,
            expires,
            admit_until,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_leaves_the_full_run_budget_and_recovery_margin() {
        let window = Window::new(1000.75, 1001.0).unwrap();
        assert_eq!((window.created, window.expires), (1000, 87400));
        assert_eq!(window.admit_until, 40600);
        assert!(Window::new(1000.75, 40599.9).is_ok());
        assert!(Window::new(1000.75, 40600.0).is_err());
        assert_eq!(
            window.expires - window.admit_until,
            super::super::wire::MAX_RUN_TIMEOUT.as_secs() as i64 + RECOVERY_MARGIN.as_secs() as i64
        );
    }

    #[test]
    fn unknown_future_and_unrepresentable_epochs_refuse() {
        for (created, now) in [
            (f64::NAN, 1.0),
            (1.0, f64::NAN),
            (f64::INFINITY, f64::INFINITY),
            (-1.0, 1.0),
            (2.0, 1.0),
            ((1_u64 << 53) as f64, (1_u64 << 53) as f64),
        ] {
            assert!(Window::new(created, now).is_err());
        }
    }
}
