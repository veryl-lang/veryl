#[cfg(feature = "git-gitoxide")]
mod gitoxide;
#[cfg(feature = "git-gitoxide")]
pub use gitoxide::Git;
#[cfg(all(feature = "git-command", not(feature = "git-gitoxide")))]
mod command;
#[cfg(all(feature = "git-command", not(feature = "git-gitoxide")))]
pub use command::Git;
