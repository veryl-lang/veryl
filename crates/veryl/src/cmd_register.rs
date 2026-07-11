use crate::OptRegister;
use log::{info, warn};
use miette::{IntoDiagnostic, Result, bail};
use std::io::{self, IsTerminal, Write};
use veryl_metadata::Metadata;

/// Default registry endpoint (overridden by `registry_base`).
const DEFAULT_REGISTRY_URL: &str = "https://registry.veryl-lang.org";

pub struct CmdRegister {
    opt: OptRegister,
}

impl CmdRegister {
    pub fn new(opt: OptRegister) -> Self {
        Self { opt }
    }

    /// Explicit `veryl register`: register regardless of `[publish] register`,
    /// so an already-published project can opt in without bumping a version.
    pub fn exec(&self, metadata: &Metadata) -> Result<bool> {
        let Some(info) = RegisterInfo::from_metadata(metadata)? else {
            bail!(
                "no repository to register; set `repository` under [project] in Veryl.toml to a git URL like `https://github.com/<owner>/<repo>` or `https://gitlab.com/<group>/<repo>`"
            );
        };

        let confirmed = if self.opt.yes {
            true
        } else if io::stdin().is_terminal() && io::stderr().is_terminal() {
            prompt_register(&info)
        } else {
            bail!("refusing to register non-interactively; pass --yes to confirm");
        };

        if confirmed {
            warn_unknown_categories(&metadata.project.categories);
            submit(&info)?;
        } else {
            info!("Registration cancelled");
        }
        Ok(true)
    }
}

/// Optionally register at publish time, driven by `[publish] register` in
/// Veryl.toml: `true` registers automatically, `false` never, unset asks once.
/// Registration is a convenience and never fails the publish.
pub fn maybe_register(metadata: &Metadata) {
    if metadata.publish.register == Some(false) {
        return;
    }

    let info = match RegisterInfo::from_metadata(metadata) {
        Ok(Some(info)) => info,
        Ok(None) => {
            if metadata.publish.register == Some(true) {
                warn!(
                    "[publish] register = true, but `[project] repository` is not a valid git repository URL; skipping registration"
                );
            }
            return;
        }
        Err(x) => {
            warn!("Registry registration skipped: {x}");
            return;
        }
    };

    let should = match metadata.publish.register {
        Some(true) => true,
        Some(false) => return, // handled above; kept for exhaustiveness
        None => {
            if io::stdin().is_terminal() && io::stderr().is_terminal() {
                prompt_register(&info)
            } else {
                info!(
                    "Skipping registry registration; set `register = true` or `false` under [publish] in Veryl.toml, or run `veryl register`"
                );
                return;
            }
        }
    };

    if should {
        warn_unknown_categories(&metadata.project.categories);
        if let Err(x) = submit(&info) {
            warn!("Registry registration failed: {x}");
        }
    }
}

/// What is submitted to the registry.
struct RegisterInfo {
    /// Canonical `<host>/<path>`, e.g. `github.com/<owner>/<repo>`.
    repo: String,
    name: String,
    version: Option<String>,
}

impl RegisterInfo {
    fn from_metadata(metadata: &Metadata) -> Result<Option<Self>> {
        // The author's declared `[project] repository`, not the git `origin`
        // remote: origin varies with the checkout (forks, mirrors) and could
        // leak an unintended URL.
        let Some(url) = &metadata.project.repository else {
            return Ok(None);
        };
        let Some(repo) = repo_slug(url) else {
            return Ok(None);
        };
        Ok(Some(Self {
            repo,
            name: metadata.project.name.clone(),
            version: metadata.project.version.as_ref().map(|v| v.to_string()),
        }))
    }
}

fn prompt_register(info: &RegisterInfo) -> bool {
    let mut err = io::stderr();
    let _ = writeln!(
        err,
        "  (set `register = true` or `false` under [publish] in Veryl.toml to skip this prompt)"
    );
    let _ = write!(err, "Register {} to the Veryl registry? [y/N]: ", info.repo);
    let _ = err.flush();

    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Registry base URL, overridable via `VERYL_REGISTRY_URL`.
fn registry_base() -> String {
    std::env::var("VERYL_REGISTRY_URL").unwrap_or_else(|_| DEFAULT_REGISTRY_URL.to_string())
}

fn http_client() -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
}

#[derive(serde::Deserialize)]
struct CategoryEntry {
    slug: String,
}

/// Warn about categories the registry will silently drop. The vocabulary is
/// fetched at runtime (the registry owns it); a fetch failure just skips the
/// check, never blocking registration.
fn warn_unknown_categories(categories: &[String]) {
    if categories.is_empty() {
        return;
    }
    let base = registry_base();
    let url = format!("{}/categories.json", base.trim_end_matches('/'));
    let known: std::collections::HashSet<String> = match http_client()
        .ok()
        .and_then(|c| c.get(&url).send().ok())
        .and_then(|r| r.error_for_status().ok())
        .and_then(|r| r.json::<Vec<CategoryEntry>>().ok())
    {
        Some(list) => list.into_iter().map(|c| c.slug.to_lowercase()).collect(),
        None => return,
    };
    // An empty vocabulary means we could not meaningfully validate (e.g. a stale
    // or misconfigured endpoint); skip rather than flag every category.
    if known.is_empty() {
        return;
    }
    for c in categories {
        if !known.contains(&c.trim().to_lowercase()) {
            warn!(
                "category `{c}` is not a recognized registry category and will be ignored; see {base} for the list"
            );
        }
    }
}

fn submit(info: &RegisterInfo) -> Result<()> {
    let base = registry_base();
    let url = format!("{}/-/submit", base.trim_end_matches('/'));

    // Registration does not push. The published version becomes visible only
    // after its revision is pushed; the registry then picks it up on the next crawl.
    info!(
        "Registering {}. If the published version is not pushed yet, run `git push`; the registry will pick it up on the next crawl.",
        info.repo
    );

    let mut payload = serde_json::json!({ "repo": info.repo, "name": info.name });
    if let Some(version) = &info.version {
        payload["version"] = serde_json::Value::String(version.clone());
    }

    // A non-2xx status is not an error by itself, so `error_for_status` is what
    // surfaces the registry's rejection (e.g. a 400).
    http_client()
        .into_diagnostic()?
        .post(&url)
        .json(&payload)
        .send()
        .into_diagnostic()?
        .error_for_status()
        .into_diagnostic()?;

    info!("Registered {} with the Veryl registry", info.repo);
    info!(
        "Docs will appear at {}/{} after the next crawl",
        base.trim_end_matches('/'),
        info.repo
    );
    Ok(())
}

/// Extract the registry key `<host>/<path>` from a `[project] repository` URL,
/// or `None` if it is not a recognizable `<host>/<owner>/<repo>`. Any git host
/// and any nesting depth (GitLab subgroups) are accepted; the registry
/// re-validates the key on intake.
fn repo_slug(url: &str) -> Option<String> {
    let url = url.trim();
    // Drop any query string / fragment before parsing the path.
    let url = url.split(['?', '#']).next().unwrap_or(url);
    let url = url.strip_suffix('/').unwrap_or(url);
    let url = url.strip_suffix(".git").unwrap_or(url);

    // scp-like syntax: git@host:owner/repo
    let rest = if let Some(after) = url.strip_prefix("git@") {
        let (host, path) = after.split_once(':')?;
        format!("{host}/{path}")
    } else {
        // scheme://[user@]host/path -> host/path
        let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
        after_scheme
            .split_once('@')
            .map(|(_, r)| r.to_string())
            .unwrap_or_else(|| after_scheme.to_string())
    };

    let mut segments = rest.split('/').filter(|s| !s.is_empty());
    // Hostnames are case-insensitive; normalize so case variants key one entry.
    let host = segments.next()?.to_ascii_lowercase();
    // Require a dot so a bare path segment can't masquerade as a host.
    if !host.contains('.') {
        return None;
    }
    let path: Vec<&str> = segments.collect();
    // At least `<owner>/<repo>`; a lone user/group page is not a repository.
    if path.len() < 2 {
        return None;
    }
    Some(format!("{host}/{}", path.join("/")))
}

#[cfg(test)]
mod tests {
    use super::repo_slug;

    #[test]
    fn parses_https() {
        assert_eq!(
            repo_slug("https://github.com/alice/fifo.git").as_deref(),
            Some("github.com/alice/fifo")
        );
        assert_eq!(
            repo_slug("https://github.com/alice/fifo").as_deref(),
            Some("github.com/alice/fifo")
        );
    }

    #[test]
    fn parses_scp_and_ssh() {
        assert_eq!(
            repo_slug("git@github.com:alice/fifo.git").as_deref(),
            Some("github.com/alice/fifo")
        );
        assert_eq!(
            repo_slug("ssh://git@github.com/alice/fifo").as_deref(),
            Some("github.com/alice/fifo")
        );
    }

    #[test]
    fn host_is_case_insensitive() {
        assert_eq!(
            repo_slug("https://GitHub.com/alice/fifo").as_deref(),
            Some("github.com/alice/fifo")
        );
    }

    #[test]
    fn accepts_other_hosts_and_nesting() {
        assert_eq!(
            repo_slug("https://gitlab.com/alice/fifo").as_deref(),
            Some("gitlab.com/alice/fifo")
        );
        assert_eq!(
            repo_slug("https://gitlab.com/group/subgroup/fifo").as_deref(),
            Some("gitlab.com/group/subgroup/fifo")
        );
        assert_eq!(
            repo_slug("https://codeberg.org/alice/fifo.git").as_deref(),
            Some("codeberg.org/alice/fifo")
        );
        assert_eq!(
            repo_slug("git@gitlab.com:group/sub/proj.git").as_deref(),
            Some("gitlab.com/group/sub/proj")
        );
    }

    #[test]
    fn rejects_non_host_or_too_shallow() {
        // No hostname (no dot).
        assert_eq!(repo_slug("https://localhost/alice/fifo"), None);
        // Only one path segment: a user/group page, not a repo.
        assert_eq!(repo_slug("https://github.com/alice"), None);
        // Host only.
        assert_eq!(repo_slug("https://github.com"), None);
    }

    #[test]
    fn strips_query_and_fragment() {
        assert_eq!(
            repo_slug("https://github.com/alice/fifo#readme").as_deref(),
            Some("github.com/alice/fifo")
        );
        assert_eq!(
            repo_slug("https://gitlab.com/group/fifo.git?tab=readme").as_deref(),
            Some("gitlab.com/group/fifo")
        );
    }
}
