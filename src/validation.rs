//! Extensible model validation primitives for SDK consumers.
//!
//! The first supported format is binary glTF (`.glb`). The API returns typed
//! issues instead of only a boolean so later SDK slices can add checks for
//! dimensions, animation names, materials, licensing metadata, or manifest
//! signatures without changing the basic call shape.

use std::{fmt, fs, io, path::Path};

pub const GLB_HEADER_LEN: usize = 12;
pub const GLB_MAGIC: [u8; 4] = *b"glTF";
pub const SUPPORTED_GLB_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFormat {
    Glb,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelValidationIssue {
    FileTooLarge {
        max_len: usize,
        actual_len: usize,
    },
    TooSmall {
        min_len: usize,
        actual_len: usize,
    },
    InvalidMagic {
        expected: [u8; 4],
        found: [u8; 4],
    },
    UnsupportedGlbVersion {
        expected: u32,
        found: u32,
    },
    DeclaredLengthTooSmall {
        min_len: usize,
        declared_len: usize,
    },
    DeclaredLengthExceedsBuffer {
        declared_len: usize,
        actual_len: usize,
    },
    DeclaredLengthMismatch {
        declared_len: usize,
        actual_len: usize,
    },
}

impl fmt::Display for ModelValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileTooLarge {
                max_len,
                actual_len,
            } => write!(
                f,
                "file is too large: {actual_len} bytes > {max_len} byte limit"
            ),
            Self::TooSmall {
                min_len,
                actual_len,
            } => write!(
                f,
                "file is too small: {actual_len} bytes < {min_len} byte GLB header"
            ),
            Self::InvalidMagic { expected, found } => {
                write!(f, "invalid magic: expected {expected:?}, found {found:?}")
            }
            Self::UnsupportedGlbVersion { expected, found } => {
                write!(
                    f,
                    "unsupported GLB version: expected {expected}, found {found}"
                )
            }
            Self::DeclaredLengthTooSmall {
                min_len,
                declared_len,
            } => write!(
                f,
                "declared length is too small: {declared_len} bytes < {min_len} byte GLB header"
            ),
            Self::DeclaredLengthExceedsBuffer {
                declared_len,
                actual_len,
            } => write!(
                f,
                "declared length exceeds buffer: {declared_len} bytes > {actual_len} bytes"
            ),
            Self::DeclaredLengthMismatch {
                declared_len,
                actual_len,
            } => write!(
                f,
                "declared length mismatch: declared {declared_len} bytes, actual {actual_len} bytes"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlbValidationRules {
    pub max_bytes: Option<usize>,
    pub allow_trailing_bytes: bool,
}

impl Default for GlbValidationRules {
    fn default() -> Self {
        Self {
            max_bytes: None,
            allow_trailing_bytes: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelValidationReport {
    pub format: ModelFormat,
    pub byte_len: usize,
    pub declared_len: Option<usize>,
    issues: Vec<ModelValidationIssue>,
}

impl ModelValidationReport {
    pub fn is_valid(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn issues(&self) -> &[ModelValidationIssue] {
        &self.issues
    }

    pub fn into_issues(self) -> Vec<ModelValidationIssue> {
        self.issues
    }
}

pub fn validate_glb_bytes(bytes: &[u8], rules: &GlbValidationRules) -> ModelValidationReport {
    let mut issues = Vec::new();
    if let Some(max_len) = rules.max_bytes
        && bytes.len() > max_len
    {
        issues.push(ModelValidationIssue::FileTooLarge {
            max_len,
            actual_len: bytes.len(),
        });
    }

    if bytes.len() < GLB_HEADER_LEN {
        issues.push(ModelValidationIssue::TooSmall {
            min_len: GLB_HEADER_LEN,
            actual_len: bytes.len(),
        });
        return ModelValidationReport {
            format: ModelFormat::Unknown,
            byte_len: bytes.len(),
            declared_len: None,
            issues,
        };
    }

    let found_magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    let format = if found_magic == GLB_MAGIC {
        ModelFormat::Glb
    } else {
        issues.push(ModelValidationIssue::InvalidMagic {
            expected: GLB_MAGIC,
            found: found_magic,
        });
        ModelFormat::Unknown
    };

    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != SUPPORTED_GLB_VERSION {
        issues.push(ModelValidationIssue::UnsupportedGlbVersion {
            expected: SUPPORTED_GLB_VERSION,
            found: version,
        });
    }

    let declared_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if declared_len < GLB_HEADER_LEN {
        issues.push(ModelValidationIssue::DeclaredLengthTooSmall {
            min_len: GLB_HEADER_LEN,
            declared_len,
        });
    }
    if declared_len > bytes.len() {
        issues.push(ModelValidationIssue::DeclaredLengthExceedsBuffer {
            declared_len,
            actual_len: bytes.len(),
        });
    }
    if !rules.allow_trailing_bytes && declared_len != bytes.len() {
        issues.push(ModelValidationIssue::DeclaredLengthMismatch {
            declared_len,
            actual_len: bytes.len(),
        });
    }

    ModelValidationReport {
        format,
        byte_len: bytes.len(),
        declared_len: Some(declared_len),
        issues,
    }
}

pub fn validate_glb_file(
    path: impl AsRef<Path>,
    rules: &GlbValidationRules,
) -> io::Result<ModelValidationReport> {
    let bytes = fs::read(path)?;
    Ok(validate_glb_bytes(&bytes, rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glb_header(declared_len: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&GLB_MAGIC);
        bytes.extend_from_slice(&SUPPORTED_GLB_VERSION.to_le_bytes());
        bytes.extend_from_slice(&declared_len.to_le_bytes());
        bytes
    }

    #[test]
    fn valid_glb_header_passes_default_rules() {
        let bytes = glb_header(GLB_HEADER_LEN as u32);
        let report = validate_glb_bytes(&bytes, &GlbValidationRules::default());
        assert!(report.is_valid());
        assert_eq!(report.format, ModelFormat::Glb);
        assert_eq!(report.declared_len, Some(GLB_HEADER_LEN));
    }

    #[test]
    fn reports_multiple_header_issues() {
        let mut bytes = glb_header(64);
        bytes[0] = b'X';
        bytes[4..8].copy_from_slice(&1_u32.to_le_bytes());

        let report = validate_glb_bytes(&bytes, &GlbValidationRules::default());
        assert!(!report.is_valid());
        assert!(
            report
                .issues()
                .iter()
                .any(|issue| { matches!(issue, ModelValidationIssue::InvalidMagic { .. }) })
        );
        assert!(
            report.issues().iter().any(|issue| {
                matches!(issue, ModelValidationIssue::UnsupportedGlbVersion { .. })
            })
        );
        assert!(report.issues().iter().any(|issue| {
            matches!(
                issue,
                ModelValidationIssue::DeclaredLengthExceedsBuffer { .. }
            )
        }));
    }

    #[test]
    fn strict_rules_can_reject_trailing_bytes_and_large_files() {
        let mut bytes = glb_header(GLB_HEADER_LEN as u32);
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        let rules = GlbValidationRules {
            max_bytes: Some(GLB_HEADER_LEN),
            allow_trailing_bytes: false,
        };

        let report = validate_glb_bytes(&bytes, &rules);
        assert!(!report.is_valid());
        assert!(
            report
                .issues()
                .iter()
                .any(|issue| matches!(issue, ModelValidationIssue::FileTooLarge { .. }))
        );
        assert!(
            report.issues().iter().any(|issue| {
                matches!(issue, ModelValidationIssue::DeclaredLengthMismatch { .. })
            })
        );
    }

    #[test]
    fn issue_display_is_human_readable() {
        let issue = ModelValidationIssue::DeclaredLengthExceedsBuffer {
            declared_len: 64,
            actual_len: 12,
        };
        assert!(issue.to_string().contains("declared length exceeds buffer"));
    }
}
