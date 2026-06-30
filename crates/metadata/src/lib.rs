mod build;
mod build_info;
mod doc;
mod format;
mod git;
mod lint;
mod lockfile;
mod lockfile_compat;
mod metadata;
mod metadata_error;
mod metadata_output;
mod project;
mod pubfile;
mod publish;
mod synth;
mod test;
#[cfg(test)]
mod tests;
pub use build::{Build, BuiltinType, ClockType, FilelistType, ResetType, SourceMapTarget, Target};
pub use build_info::BuildInfo;
pub use doc::Doc;
pub use format::{Format, NewlineStyle};
pub use git::Git;
pub use lint::{Case, Lint};
pub use lockfile::{LockSource, Lockfile};
pub use metadata::{BumpKind, Metadata, UrlPath};
pub use metadata_error::MetadataError;
pub use metadata_output::{
    MetadataDependencyV2, MetadataOutputV2, MetadataProjectV2, MetadataSourceV2,
};
pub use project::Project;
pub use pubfile::{Pubfile, Release};
pub use publish::Publish;
pub use semver;
pub use synth::{Library, Synth};
pub use test::{SimType, Test, WaveFormFormat, WaveFormTarget};

include!(concat!(env!("OUT_DIR"), "/veryl_version.rs"));
