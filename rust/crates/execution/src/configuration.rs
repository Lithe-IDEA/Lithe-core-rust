//! Run-configuration schemas, layered overrides, and deterministic generation.

mod project_toolchain;
pub use project_toolchain::{project_toolchain, ProjectToolchainRequest};

use super::generated_layer;
use super::types::{Confidence, Execution, RunCategory};
use lithe_platform::paths::{
    invalid_native_relative_path, invalid_portable_relative_path, slash_separated_path,
};
use lithe_platform::Platform;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

mod document;
mod editing;
mod generation;
mod launch;
mod layers;
mod merge;
mod requests;
mod resolve;
mod validation;

pub use document::*;
pub use editing::*;
pub use generation::*;
pub use launch::*;
pub(crate) use layers::*;
pub(crate) use merge::*;
pub use requests::*;
pub use resolve::*;
pub use validation::*;

const VERSION: u32 = 2;

const LEGACY_VERSION: u32 = 1;

/// Local toolchains and `project.json` are separate documents that happen
/// to live under `.lithe`. Their schema did not change with run-config v2, so
/// they keep their own version and must not be validated against `VERSION`.
const SIDECAR_VERSION: u32 = 1;
