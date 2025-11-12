//! Human-readable size formatting and parsing utilities

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("Invalid size format: {0}")]
    InvalidFormat(String),

    #[error("Invalid number: {0}")]
    InvalidNumber(#[from] std::num::ParseIntError),

    #[error("Invalid unit: {0}")]
    InvalidUnit(String),
}

/// Byte size wrapper with human-readable parsing
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn to_human_readable(&self) -> String {
        const UNITS: &[(&str, u64)] = &[
            ("B", 1),
            ("KB", 1024),
            ("MB", 1024 * 1024),
            ("GB", 1024 * 1024 * 1024),
            ("TB", 1024 * 1024 * 1024 * 1024),
        ];

        for (i, &(unit, divisor)) in UNITS.iter().enumerate().rev() {
            if self.0 >= divisor {
                let value = self.0 / divisor;
                let remainder = self.0 % divisor;

                if remainder == 0 || i == 0 {
                    return format!("{}{}", value, unit);
                } else {
                    let decimal = (remainder * 10 / divisor) as u64;
                    if decimal > 0 {
                        return format!("{}.{}{}", value, decimal, unit);
                    }
                    return format!("{}{}", value, unit);
                }
            }
        }

        format!("{}B", self.0)
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ByteSizeVisitor;

        impl<'de> serde::de::Visitor<'de> for ByteSizeVisitor {
            type Value = ByteSize;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a byte size as string (e.g., \"5MB\", \"1GB\") or integer")
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ByteSize(v))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                v.parse::<ByteSize>().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(ByteSizeVisitor)
    }
}

impl FromStr for ByteSize {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().to_uppercase();

        // Try to parse as plain number first
        if let Ok(num) = s.parse::<u64>() {
            return Ok(ByteSize(num));
        }

        // Parse with unit suffix
        let (num_str, unit) = if let Some(pos) = s.find(|c: char| !c.is_ascii_digit()) {
            (&s[..pos], &s[pos..])
        } else {
            return Err(ParseError::InvalidFormat(s.to_string()));
        };

        let num: u64 = num_str.parse()?;

        let multiplier = match unit.trim() {
            "B" => 1,
            "K" | "KB" | "KIB" => 1024,
            "M" | "MB" | "MIB" => 1024 * 1024,
            "G" | "GB" | "GIB" => 1024 * 1024 * 1024,
            "T" | "TB" | "TIB" => 1024 * 1024 * 1024 * 1024,
            _ => return Err(ParseError::InvalidUnit(unit.to_string())),
        };

        Ok(ByteSize(num * multiplier))
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_human_readable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bytes() {
        assert_eq!("1024".parse::<ByteSize>().unwrap().as_u64(), 1024);
        assert_eq!("1KB".parse::<ByteSize>().unwrap().as_u64(), 1024);
        assert_eq!("1K".parse::<ByteSize>().unwrap().as_u64(), 1024);
    }

    #[test]
    fn test_parse_megabytes() {
        assert_eq!("5MB".parse::<ByteSize>().unwrap().as_u64(), 5 * 1024 * 1024);
        assert_eq!("5M".parse::<ByteSize>().unwrap().as_u64(), 5 * 1024 * 1024);
        assert_eq!("5MiB".parse::<ByteSize>().unwrap().as_u64(), 5 * 1024 * 1024);
    }

    #[test]
    fn test_parse_gigabytes() {
        assert_eq!("50GB".parse::<ByteSize>().unwrap().as_u64(), 50 * 1024 * 1024 * 1024);
        assert_eq!("1G".parse::<ByteSize>().unwrap().as_u64(), 1024 * 1024 * 1024);
    }

    #[test]
    fn test_parse_terabytes() {
        assert_eq!("1TB".parse::<ByteSize>().unwrap().as_u64(), 1024 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_to_human_readable() {
        assert_eq!(ByteSize(1024).to_human_readable(), "1KB");
        assert_eq!(ByteSize(5 * 1024 * 1024).to_human_readable(), "5MB");
        assert_eq!(ByteSize(50 * 1024 * 1024 * 1024).to_human_readable(), "50GB");
    }

    #[test]
    fn test_deserialize_string() {
        let json = r#"{"size": "10MB"}"#;
        #[derive(Deserialize)]
        struct TestStruct {
            size: ByteSize,
        }
        let parsed: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.size.as_u64(), 10 * 1024 * 1024);
    }

    #[test]
    fn test_deserialize_number() {
        let json = r#"{"size": 1024}"#;
        #[derive(Deserialize)]
        struct TestStruct {
            size: ByteSize,
        }
        let parsed: TestStruct = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.size.as_u64(), 1024);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", ByteSize(1024)), "1KB");
        assert_eq!(format!("{}", ByteSize(5 * 1024 * 1024)), "5MB");
    }
}
