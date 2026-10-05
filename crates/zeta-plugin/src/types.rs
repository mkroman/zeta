use serde::{Deserialize, Serialize};

/// The settings type for plugins that have no configuration.
///
/// Used as the settings type of plugins that do not read any values from their
/// `[plugins.<name>]` configuration section. Only the host-managed `enabled` key is read; any
/// other key is ignored, but reported as a warning by the host — likely a typo, or a setting
/// that no longer exists.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct NoSettings {}
