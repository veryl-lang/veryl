#[cfg(feature = "git-gitoxide")]
mod gitoxide;
#[cfg(feature = "git-gitoxide")]
pub use gitoxide::Git;
#[cfg(feature = "git-command")]
mod command;
#[cfg(feature = "git-command")]
pub use command::Git;
