//! Current-version schema validation, separate from migration admission.
use super::*;
pub(crate) fn validate_current(connection: &Connection) -> Result<(), Error> {
    ci_state::validate(connection)
}
