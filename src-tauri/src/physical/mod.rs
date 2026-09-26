//! Physical contracts and Stage 2 durable/trusted environmental facts, never authority.
//!
//! No task authority, executable session, protocol, adapter or native I/O lives here.
//! Successfully validating a claim does not authenticate its producer or qualify a body.
#![allow(dead_code)] // Deliberately has no runtime consumers until later stages.

use crate::error::{AppError, AppResult};

fn require(condition: bool, message: &str) -> AppResult<()> {
    if condition {
        Ok(())
    } else {
        Err(AppError::InvalidInput(message.into()))
    }
}

// Keep semantic validation identical for constructed claims and wire input. Fields
// remain data, not permits; callers must validate again after changing a claim.
macro_rules! claim {
    ($(#[$meta:meta])* $name:ident { $($(#[$attr:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, serde::Serialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub(crate) struct $name { $( $(#[$attr])* pub $field: $ty, )* }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase", deny_unknown_fields)]
                struct Fields { $( $(#[$attr])* $field: $ty, )* }
                let fields = <Fields as serde::Deserialize>::deserialize(deserializer)?;
                let value = Self { $( $field: fields.$field, )* };
                value.validate().map_err(serde::de::Error::custom)?;
                Ok(value)
            }
        }
    };
}

pub(crate) mod binding;
pub(crate) mod contracts;
pub(crate) mod store;
pub(crate) mod values;

#[cfg(test)]
mod tests;
