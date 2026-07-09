use crate::OptNew;
use log::info;
use miette::{IntoDiagnostic, Result, bail};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use veryl_metadata::{Git, Metadata};

pub struct CmdNew {
    opt: OptNew,
}

impl CmdNew {
    pub fn new(opt: OptNew) -> Self {
        Self { opt }
    }

    pub fn exec(&self) -> Result<bool> {
        if self.opt.path.exists() {
            bail!("path \"{}\" exists", self.opt.path.to_string_lossy());
        }

        if self.opt.component {
            return self.create_component();
        }

        if let Some(name) = self.opt.path.file_name() {
            let name = name.to_string_lossy();

            let toml = Metadata::create_default_toml(&name).into_diagnostic()?;
            let toml_path = self.opt.path.join("Veryl.toml");

            fs::create_dir_all(&self.opt.path).into_diagnostic()?;
            let mut file = File::create(toml_path).into_diagnostic()?;
            write!(file, "{toml}").into_diagnostic()?;
            file.flush().into_diagnostic()?;

            let src_path = self.opt.path.join("src");
            fs::create_dir_all(&src_path).into_diagnostic()?;

            if Git::exists() {
                let gitignore = Metadata::create_default_gitignore();
                let gitignore_path = self.opt.path.join(".gitignore");

                let mut file = File::create(&gitignore_path).into_diagnostic()?;
                write!(file, "{gitignore}").into_diagnostic()?;
                file.flush().into_diagnostic()?;

                Git::init(&self.opt.path)?;
            }

            info!("Created \"{name}\" project");
        } else {
            bail!("path \"{}\" is not valid", self.opt.path.to_string_lossy());
        }

        Ok(true)
    }
}

fn to_pascal_case(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|s| {
            let mut chars = s.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

impl CmdNew {
    fn create_component(&self) -> Result<bool> {
        let Some(name) = self.opt.path.file_name() else {
            bail!("path \"{}\" is not valid", self.opt.path.to_string_lossy());
        };
        let name = name.to_string_lossy();
        // The name becomes a crate name and is embedded in
        // `$comp::<name>` and `veryl_component_export!`, so the
        // project-name rules apply.
        veryl_metadata::check_project_name(&name).into_diagnostic()?;
        let type_name = to_pascal_case(&name);
        let version = env!("CARGO_PKG_VERSION");

        let cargo_toml = format!(
            r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
veryl-component = "{version}"

# Standalone package: keep it out of any enclosing cargo workspace.
[workspace]
"#
        );

        let lib_rs = format!(
            r#"use veryl_component::*;

/// Fires like an `always_ff` process on the connected clock: inputs
/// observe pre-edge values, outputs commit with FFs.
// Host capabilities are declared here too, e.g.
// `#[component(kind = clocked, requires(file))]` for ctx file I/O
// (enforced when the component runs as a prebuilt wasm binary).
#[derive(Component)]
#[component(kind = clocked)]
pub struct {type_name} {{
    /// Sampling clock.
    clk: ClockPort,
    // Fields typed InputPort/OutputPort/ClockPort/ResetPort become ports
    // (widths inferred from the connection), `#[param]` fields become
    // parameters, everything else is plain state (Default-initialized):
    // /// Handshake to observe.
    // valid: InputPort,
    // /// Cycle budget for the check.
    // #[param(name = "LIMIT")]
    // limit: u64,
}}

#[component_impl]
impl {type_name} {{
    fn on_clock(&mut self, ctx: &mut SimCtx) -> Result<()> {{
        let _fired = ctx.fired(self.clk);
        // let valid = ctx.read(self.valid).as_bool();
        // if mismatch {{ ctx.fail("..."); }}
        // if done {{ ctx.finish(); }}
        Ok(())
    }}

    // Other functions become zero-time testbench methods,
    // e.g. `inst.load("x.elf");`:
    // /// Load an ELF file.
    // fn load(&mut self, ctx: &mut SimCtx, path: &str) -> Result<()> {{ ... }}
}}

veryl_component_export!("{name}" => {type_name});

#[cfg(test)]
mod tests {{
    use super::*;
    use veryl_component::testing::MockSim;

    #[test]
    fn clock_runs() {{
        let mut sim = MockSim::new().clock_port("clk");
        let mut c = sim.build::<{type_name}>().unwrap();
        sim.clock(&mut c).unwrap();
        assert!(!sim.failed());
    }}
}}
"#
        );

        // A component is a cargo package listed in a Veryl project's
        // `[[components]]`: register into the enclosing project, or scaffold a
        // standalone one when there is none.
        let cwd = std::env::current_dir().into_diagnostic()?;
        let target = cwd.join(&self.opt.path);
        match Metadata::search_from(&target) {
            Ok(veryl_toml) => {
                write_crate(&self.opt.path, &cargo_toml, &lib_rs)?;
                let root = veryl_toml.parent().unwrap_or(Path::new("."));
                let rel = relative_slash_path(&target, root);
                let added = register_component(&veryl_toml, &rel, &name)?;
                let shown = veryl_toml
                    .strip_prefix(&cwd)
                    .unwrap_or(veryl_toml.as_path());
                info!("Created \"{name}\" component");
                if added {
                    info!("Registered $comp::{name} in {}", shown.display());
                } else {
                    info!("$comp::{name} is already registered in {}", shown.display());
                }
                info!("Instantiate it in a #[test] module as $comp::{name}");
            }
            Err(_) => self.create_component_project(&name, &cargo_toml, &lib_rs)?,
        }

        Ok(true)
    }

    /// Scaffolds a self-contained project whose `examples/` testbench runs with
    /// `veryl test` and which other projects can pull in as a dependency.
    fn create_component_project(&self, name: &str, cargo_toml: &str, lib_rs: &str) -> Result<()> {
        let root = &self.opt.path;
        write_crate(&root.join("component"), cargo_toml, lib_rs)?;

        let veryl_toml = format!(
            r#"[project]
name = "{name}"
version = "0.1.0"

[[components]]
path = "component"
# optional committed prebuilt for cargo-less consumers, generated by `veryl publish`:
# wasm = "prebuilt/{name}.wasm"
"#
        );
        write_file(&root.join("Veryl.toml"), &veryl_toml)?;

        // `examples/` is analyzed automatically and excluded from the build
        // output, so its `#[test]` modules run without a `[build]` sources entry.
        let testbench = format!(
            r#"#[test({name})]
module {name} {{
    inst clk: $tb::clock_gen;
    inst dut: $comp::{name} (clk);

    initial {{
        clk.next(4);
        $finish();
    }}
}}
"#
        );
        write_file(
            &root.join("examples").join(format!("{name}.veryl")),
            &testbench,
        )?;

        if Git::exists() {
            let gitignore = format!(
                "{}\n# Component crate\n/component/target\n",
                Metadata::create_default_gitignore()
            );
            write_file(&root.join(".gitignore"), &gitignore)?;
            Git::init(root)?;
        }

        info!("Created \"{name}\" component project");
        info!("Run `veryl test` in {} to try it", root.display());
        Ok(())
    }
}

fn write_crate(dir: &Path, cargo_toml: &str, lib_rs: &str) -> Result<()> {
    write_file(&dir.join("Cargo.toml"), cargo_toml)?;
    write_file(&dir.join("src").join("lib.rs"), lib_rs)
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).into_diagnostic()?;
    }
    let mut file = File::create(path).into_diagnostic()?;
    write!(file, "{content}").into_diagnostic()?;
    file.flush().into_diagnostic()
}

/// `target` relative to `project_root`, with forward slashes for the manifest.
fn relative_slash_path(target: &Path, project_root: &Path) -> String {
    target
        .strip_prefix(project_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/")
}

/// A relative path canonicalized for comparison.
fn normalize_rel(p: &Path) -> String {
    p.components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Registers `rel` in `veryl_toml`'s `[[components]]` unless already present;
/// returns whether it was added. Uses `FromStr`, not `Metadata::load`, to stay
/// read-only — `load` would create `.build/` in the enclosing project.
fn register_component(veryl_toml: &Path, rel: &str, name: &str) -> Result<bool> {
    let mut content = fs::read_to_string(veryl_toml).into_diagnostic()?;
    let metadata: Metadata = content.parse().into_diagnostic()?;
    let target = normalize_rel(Path::new(rel));
    if metadata
        .components
        .iter()
        .any(|c| normalize_rel(&c.path) == target)
    {
        return Ok(false);
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&format!(
        "\n[[components]]\npath = \"{rel}\"\n# optional committed prebuilt for cargo-less consumers, generated by `veryl publish`:\n# wasm = \"prebuilt/{name}.wasm\"\n"
    ));
    fs::write(veryl_toml, content).into_diagnostic()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component_cmd(name: &str) -> CmdNew {
        CmdNew::new(OptNew {
            path: std::env::temp_dir().join(name),
            component: true,
        })
    }

    #[test]
    fn component_name_is_validated() {
        for bad in ["bad-name", "1bad", "__reserved", "bad name"] {
            let cmd = component_cmd(bad);
            assert!(cmd.create_component().is_err());
            assert!(!cmd.opt.path.exists());
        }
    }

    #[test]
    fn pascal_case_conversion() {
        assert_eq!(to_pascal_case("rv_iss"), "RvIss");
        assert_eq!(to_pascal_case("golden"), "Golden");
    }

    #[test]
    fn relative_slash_path_strips_root() {
        let root = Path::new("/proj");
        assert_eq!(
            relative_slash_path(Path::new("/proj/checkers/foo"), root),
            "checkers/foo"
        );
        // A target outside the project falls back to the given path.
        assert_eq!(
            relative_slash_path(Path::new("/elsewhere/foo"), root),
            "/elsewhere/foo"
        );
    }

    #[test]
    fn register_component_appends_and_is_idempotent() {
        let dir = std::env::temp_dir().join("veryl_new_register_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let toml = dir.join("Veryl.toml");
        fs::write(&toml, "[project]\nname = \"p\"\nversion = \"0.1.0\"\n").unwrap();

        assert!(register_component(&toml, "checkers/foo", "foo").unwrap());
        let after = fs::read_to_string(&toml).unwrap();
        assert!(after.contains("[[components]]"), "{after}");
        assert!(after.contains("path = \"checkers/foo\""), "{after}");

        // A second registration of the same path is a no-op, and a
        // differently-spelled path for the same crate is recognized too.
        assert!(!register_component(&toml, "checkers/foo", "foo").unwrap());
        assert!(!register_component(&toml, "./checkers/foo", "foo").unwrap());
        assert!(!register_component(&toml, "checkers/foo/", "foo").unwrap());
        assert_eq!(
            fs::read_to_string(&toml)
                .unwrap()
                .matches("[[components]]")
                .count(),
            1
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn component_project_scaffolds_structure() {
        let root = std::env::temp_dir().join("veryl_new_project_test/my_checker");
        let _ = fs::remove_dir_all(&root);
        let cmd = CmdNew::new(OptNew {
            path: root.clone(),
            component: true,
        });
        cmd.create_component_project("my_checker", "cargo", "lib")
            .unwrap();

        assert!(root.join("component/Cargo.toml").is_file());
        assert!(root.join("component/src/lib.rs").is_file());
        assert!(root.join("examples/my_checker.veryl").is_file());
        let toml = fs::read_to_string(root.join("Veryl.toml")).unwrap();
        assert!(toml.contains("path = \"component\""), "{toml}");
        let tb = fs::read_to_string(root.join("examples/my_checker.veryl")).unwrap();
        assert!(tb.contains("$comp::my_checker"), "{tb}");
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }
}
