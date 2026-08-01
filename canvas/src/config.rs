//! Typed values owned by the canvas configuration domain.

use std::fmt;

use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Deserializer, Serialize};

const ROWS: &str = "canvas.pack.rows";
const DEFAULT_ROWS: u32 = 40;

/// Effective canvas configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    pack: Pack,
}

impl Config {
    /// Admit one row limit.
    ///
    /// # Errors
    ///
    /// Refuses zero or a value wider than the portable `u32` configuration
    /// range and names `canvas.pack.rows`.
    pub fn new(rows: u64) -> Result<Self, ConfigError> {
        Ok(Self {
            pack: Pack {
                rows: Rows::new(rows)?,
            },
        })
    }

    /// Runtime-ready ranked-row limit.
    #[must_use]
    pub const fn rows(self) -> usize {
        self.pack.rows.get() as usize
    }

    /// Portable configured row count.
    #[must_use]
    pub const fn row_count(self) -> u32 {
        self.pack.rows.get()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new(u64::from(DEFAULT_ROWS)).expect("the compiled canvas row count is positive")
    }
}

impl Serialize for Config {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("CanvasConfig", 1)?;
        state.serialize_field("pack", &self.pack)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ConfigVisitor)
    }
}

struct ConfigVisitor;

impl<'de> Visitor<'de> for ConfigVisitor {
    type Value = Config;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the canvas configuration table")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut pack = None;
        while let Some(field) = map.next_key::<String>()? {
            if field == "pack" {
                if pack.is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate configuration field canvas.pack",
                    ));
                }
                pack = Some(map.next_value::<Pack>()?);
            } else {
                map.next_value::<IgnoredAny>()?;
                return Err(serde::de::Error::custom(format!(
                    "unknown configuration field canvas.{field}"
                )));
            }
        }
        Ok(Self::Value {
            pack: pack.unwrap_or_default(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Pack {
    rows: Rows,
}

impl Default for Pack {
    fn default() -> Self {
        Self {
            rows: Rows(DEFAULT_ROWS),
        }
    }
}

impl Serialize for Pack {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("CanvasPackConfig", 1)?;
        state.serialize_field("rows", &self.rows)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for Pack {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(PackVisitor)
    }
}

struct PackVisitor;

impl<'de> Visitor<'de> for PackVisitor {
    type Value = Pack;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the canvas.pack configuration table")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut rows = None;
        while let Some(field) = map.next_key::<String>()? {
            if field == "rows" {
                if rows.is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate configuration field {ROWS}"
                    )));
                }
                let value = map.next_value::<u64>().map_err(|error| {
                    serde::de::Error::custom(format!("invalid {ROWS}: {error}"))
                })?;
                rows = Some(Rows::new(value).map_err(serde::de::Error::custom)?);
            } else {
                map.next_value::<IgnoredAny>()?;
                return Err(serde::de::Error::custom(format!(
                    "unknown configuration field canvas.pack.{field}"
                )));
            }
        }
        Ok(Pack {
            rows: rows.unwrap_or(Rows(DEFAULT_ROWS)),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Rows(u32);

impl Rows {
    fn new(value: u64) -> Result<Self, ConfigError> {
        if value == 0 {
            return Err(ConfigError::ZeroRows);
        }
        let value = u32::try_from(value).map_err(|_| ConfigError::RowsOverflow { value })?;
        Ok(Self(value))
    }

    const fn get(self) -> u32 {
        self.0
    }
}

impl Serialize for Rows {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

/// Why an effective canvas value was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    /// Packing zero rows cannot make progress.
    ZeroRows,
    /// The row count exceeded the portable configuration range.
    RowsOverflow {
        /// Exact refused value.
        value: u64,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroRows => formatter.write_str("canvas.pack.rows must be positive"),
            Self::RowsOverflow { value } => write!(
                formatter,
                "canvas.pack.rows exceeds the maximum {}: {value}",
                u32::MAX
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_explicit_rows_are_runtime_ready_and_round_trip() {
        let builtin = Config::default();
        assert_eq!(builtin.rows(), 40);
        assert_eq!(
            wire::decode::<Config>(b"{}").expect("empty canvas"),
            builtin
        );
        assert_eq!(
            wire::decode::<Config>(br#"{"pack":{}}"#).expect("empty pack"),
            builtin
        );

        let explicit: Config =
            wire::decode(br#"{"pack":{"rows":80}}"#).expect("decode canvas config");
        assert_eq!(explicit.rows(), 80);
        let bytes = wire::encode(&explicit).expect("encode canvas config");
        assert_eq!(bytes, br#"{"pack":{"rows":80}}"#);
        assert_eq!(
            wire::decode::<Config>(&bytes).expect("round trip"),
            explicit
        );
        let document = toml::to_string(&explicit).expect("encode TOML");
        assert_eq!(
            toml::from_str::<Config>(&document).expect("round-trip TOML"),
            explicit
        );
    }

    #[test]
    fn zero_overflow_and_unknown_rows_name_the_full_path() {
        for (document, path) in [
            (br#"{"pack":40}"#.as_slice(), "canvas.pack"),
            (br#"{"pack":{"rows":0}}"#.as_slice(), ROWS),
            (br#"{"pack":{"rows":4294967296}}"#.as_slice(), ROWS),
            (br#"{"pack":{"rows":"40"}}"#.as_slice(), ROWS),
            (br#"{"pack":{"other":40}}"#.as_slice(), "canvas.pack.other"),
        ] {
            let error = wire::decode::<Config>(document).expect_err("configuration must refuse");
            assert!(error.to_string().contains(path), "{error}");
        }

        let error = wire::decode::<Config>(br#"{"other":{}}"#)
            .expect_err("unknown canvas field must refuse");
        assert!(error.to_string().contains("canvas.other"), "{error}");
    }
}
