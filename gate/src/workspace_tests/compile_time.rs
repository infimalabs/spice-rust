//! Closed authority for executable Cargo compile-time producers.

use super::proof::{CargoCompileTimeAuthority, path_text};
use super::*;
use serde::Deserialize;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::visit::Visit;

const AUTHORITY_VERSION: u64 = 1;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAuthority {
    version: u64,
    complete: bool,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    environment: Vec<String>,
}

pub(super) fn authority_declaration(
    package: &str,
    metadata: &serde_json::Value,
) -> Result<Option<CompileTimeAuthorityDeclaration>, WorkspaceTestError> {
    let Some(raw) = metadata
        .get("spice")
        .and_then(|spice| spice.get("cargo-proof"))
    else {
        return Ok(None);
    };
    let raw: RawAuthority = serde_json::from_value(raw.clone()).map_err(|error| {
        non_cacheable(format!(
            "Cargo package {package:?} has invalid package.metadata.spice.cargo-proof: {error}"
        ))
    })?;
    if raw.version != AUTHORITY_VERSION {
        return Err(non_cacheable(format!(
            "Cargo package {package:?} declares unsupported compile-time authority version {}; expected {AUTHORITY_VERSION}",
            raw.version,
        )));
    }
    if !raw.complete {
        return Err(non_cacheable(format!(
            "Cargo package {package:?} compile-time authority is not declared complete"
        )));
    }
    let mut files = raw.files;
    files.sort();
    files.dedup();
    let mut environment = raw.environment;
    for name in &environment {
        validate_environment_name(package, name)?;
    }
    environment.sort();
    environment.dedup();
    Ok(Some(CompileTimeAuthorityDeclaration { files, environment }))
}

pub(super) fn compile_time_authorities(
    inventory: &Inventory,
    selected_root: &Path,
    reached_roots: &BTreeSet<PathBuf>,
    selected_paths: &[String],
    input_paths: &mut BTreeSet<String>,
) -> Result<Vec<CargoCompileTimeAuthority>, WorkspaceTestError> {
    let selected = selected_paths
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut authorities = Vec::new();
    for package in inventory
        .packages
        .iter()
        .filter(|package| reached_roots.contains(&package.root))
    {
        let executable_targets = package
            .declared_targets
            .iter()
            .filter(|target| compile_time_kind(target).is_some())
            .collect::<Vec<_>>();
        let generated_outputs = package_generated_outputs(package)?;
        if executable_targets.is_empty() {
            if !generated_outputs.is_empty() {
                return Err(non_cacheable(format!(
                    "Cargo package {:?} consumes OUT_DIR generated inputs without a custom-build producer",
                    package.name,
                )));
            }
            continue;
        }
        let declaration = package.compile_time_authority.as_ref().ok_or_else(|| {
            non_cacheable(format!(
                "Cargo package {:?} reaches executable compile-time code but has no complete package.metadata.spice.cargo-proof declaration",
                package.name,
            ))
        })?;

        let mut declared_files = declaration.files.iter().cloned().collect::<BTreeSet<_>>();
        let mut declared_environment = declaration
            .environment
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut scans = Vec::new();
        for target in &executable_targets {
            let scan = scan_target(package, target)?;
            declared_files.extend(scan.rerun_files.iter().cloned());
            declared_environment.extend(scan.rerun_environment.iter().cloned());
            scans.push((*target, scan));
        }

        if !generated_outputs.is_empty()
            && !executable_targets
                .iter()
                .any(|target| compile_time_kind(target) == Some("custom-build"))
        {
            return Err(non_cacheable(format!(
                "Cargo package {:?} consumes OUT_DIR generated inputs without a custom-build producer",
                package.name,
            )));
        }

        let mut relative_files = BTreeSet::new();
        for declared in &declared_files {
            let relative = admit_declared_file(package, selected_root, declared, &selected)?;
            input_paths.insert(relative.clone());
            relative_files.insert(relative);
        }
        for (target, scan) in scans {
            for read in &scan.file_reads {
                if !declared_files.contains(read) {
                    return Err(non_cacheable(format!(
                        "Cargo package {:?} target {} reads undeclared compile-time file {read:?}",
                        package.name,
                        target.display(),
                    )));
                }
            }
            for name in &scan.environment_reads {
                if !declared_environment.contains(name) {
                    return Err(non_cacheable(format!(
                        "Cargo package {:?} target {} reads undeclared ambient environment {name:?}",
                        package.name,
                        target.display(),
                    )));
                }
            }
            authorities.push(CargoCompileTimeAuthority {
                package: package.name.clone(),
                target: target.display(),
                kind: compile_time_kind(target)
                    .expect("compile-time targets were filtered")
                    .to_owned(),
                input_paths: relative_files.iter().cloned().collect(),
                environment: declared_environment.iter().cloned().collect(),
                generated_outputs: if compile_time_kind(target) == Some("custom-build") {
                    generated_outputs.clone()
                } else {
                    Vec::new()
                },
            });
        }
    }
    authorities.sort_by(|left, right| {
        (&left.package, &left.kind, &left.target).cmp(&(&right.package, &right.kind, &right.target))
    });
    Ok(authorities)
}

fn compile_time_kind(target: &TargetIdentity) -> Option<&'static str> {
    if target.kind.split('+').any(|kind| kind == "custom-build") {
        Some("custom-build")
    } else if target.kind.split('+').any(|kind| kind == "proc-macro") {
        Some("proc-macro")
    } else {
        None
    }
}

fn scan_target(
    package: &PackageInventory,
    target: &TargetIdentity,
) -> Result<CompileTimeScan, WorkspaceTestError> {
    let mut sources = BTreeSet::from([target.source.clone()]);
    sources.extend(
        target
            .declared_path_sources
            .iter()
            .filter(|source| source.path.extension() == Some(OsStr::new("rs")))
            .map(|source| source.path.clone()),
    );
    let mut scan = CompileTimeScan::default();
    for source in sources {
        let document = fs::read_to_string(&source).map_err(|error| {
            non_cacheable(format!(
                "read Cargo package {:?} compile-time source {}: {error}",
                package.name,
                source.display(),
            ))
        })?;
        let syntax = syn::parse_file(&document).map_err(|error| {
            non_cacheable(format!(
                "parse Cargo package {:?} compile-time source {}: {error}",
                package.name,
                source.display(),
            ))
        })?;
        let mut visitor = CompileTimeVisitor {
            package: &package.name,
            source: &source,
            scan: &mut scan,
            error: None,
        };
        visitor.visit_file(&syntax);
        if let Some(error) = visitor.error {
            return Err(error);
        }
    }
    Ok(scan)
}

#[derive(Default)]
struct CompileTimeScan {
    rerun_files: BTreeSet<String>,
    rerun_environment: BTreeSet<String>,
    file_reads: BTreeSet<String>,
    environment_reads: BTreeSet<String>,
}

struct CompileTimeVisitor<'a> {
    package: &'a str,
    source: &'a Path,
    scan: &'a mut CompileTimeScan,
    error: Option<WorkspaceTestError>,
}

impl<'ast> Visit<'ast> for CompileTimeVisitor<'_> {
    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if self.error.is_some() {
            return;
        }
        let name = invocation
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        match name.as_deref() {
            Some("println") => self.scan_println(invocation),
            Some("env" | "option_env") => self.scan_environment_macro(invocation),
            _ => {}
        }
        syn::visit::visit_macro(self, invocation);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if self.error.is_some() {
            return;
        }
        if let Expr::Path(function) = call.func.as_ref() {
            let path = rust_path(&function.path);
            if matches!(
                path.as_str(),
                "std::env::vars" | "std::env::vars_os" | "env::vars" | "env::vars_os"
            ) {
                self.refuse(format!(
                    "enumerates unbounded ambient environment through {path}"
                ));
                return;
            }
            if matches!(
                path.as_str(),
                "std::env::var" | "std::env::var_os" | "env::var" | "env::var_os"
            ) {
                match literal_call_argument(call) {
                    Some(name) => {
                        if let Err(error) = validate_environment_name(self.package, &name) {
                            self.error = Some(error);
                            return;
                        }
                        if !cargo_derived_environment(&name) {
                            self.scan.environment_reads.insert(name);
                        }
                    }
                    None => self.refuse(format!("uses dynamic ambient environment through {path}")),
                }
                return;
            }
            if process_or_network_call(&path) {
                self.refuse(format!(
                    "uses unbounded process or network dependency {path}"
                ));
                return;
            }
            if filesystem_read_call(&path) {
                match literal_call_argument(call) {
                    Some(path) => {
                        self.scan.file_reads.insert(path);
                    }
                    None => self.refuse(format!("uses dynamic filesystem dependency {path}")),
                }
                return;
            }
        }
        syn::visit::visit_expr_call(self, call);
    }
}

impl CompileTimeVisitor<'_> {
    fn scan_println(&mut self, invocation: &syn::Macro) {
        let parser = Punctuated::<Expr, syn::Token![,]>::parse_terminated;
        let Ok(arguments) = parser.parse2(invocation.tokens.clone()) else {
            if invocation.tokens.to_string().contains("rerun") {
                self.refuse("emits a dynamic or malformed Cargo rerun directive".to_owned());
            }
            return;
        };
        let Some(Expr::Lit(first)) = arguments.first() else {
            return;
        };
        let Lit::Str(value) = &first.lit else {
            return;
        };
        let value = value.value();
        if !value.contains("rerun-if") {
            return;
        }
        if arguments.len() != 1 || value.contains(['{', '}', '\n', '\r']) {
            self.refuse("emits a dynamic or multiline Cargo rerun directive".to_owned());
            return;
        }
        let directive = value
            .strip_prefix("cargo::")
            .or_else(|| value.strip_prefix("cargo:"));
        let Some((name, value)) = directive.and_then(|directive| directive.split_once('=')) else {
            self.refuse("emits a malformed Cargo rerun directive".to_owned());
            return;
        };
        if value.is_empty() {
            self.refuse("emits an empty Cargo rerun directive".to_owned());
            return;
        }
        match name {
            "rerun-if-changed" => {
                self.scan.rerun_files.insert(value.to_owned());
            }
            "rerun-if-env-changed" => {
                if let Err(error) = validate_environment_name(self.package, value) {
                    self.error = Some(error);
                } else {
                    self.scan.rerun_environment.insert(value.to_owned());
                }
            }
            _ => self.refuse(format!("emits unsupported Cargo rerun directive {name:?}")),
        }
    }

    fn scan_environment_macro(&mut self, invocation: &syn::Macro) {
        let Ok(name) = syn::parse2::<syn::LitStr>(invocation.tokens.clone()) else {
            self.refuse("uses a dynamic compile-time environment macro".to_owned());
            return;
        };
        let name = name.value();
        if let Err(error) = validate_environment_name(self.package, &name) {
            self.error = Some(error);
        } else if !cargo_derived_environment(&name) {
            self.scan.environment_reads.insert(name);
        }
    }

    fn refuse(&mut self, detail: String) {
        self.error = Some(non_cacheable(format!(
            "Cargo package {:?} compile-time source {} {detail}",
            self.package,
            self.source.display(),
        )));
    }
}

fn literal_call_argument(call: &syn::ExprCall) -> Option<String> {
    if call.args.len() != 1 {
        return None;
    }
    let Expr::Lit(argument) = call.args.first()? else {
        return None;
    };
    let Lit::Str(value) = &argument.lit else {
        return None;
    };
    Some(value.value())
}

fn rust_path(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

fn filesystem_read_call(path: &str) -> bool {
    matches!(
        path,
        "std::fs::read"
            | "std::fs::read_to_string"
            | "std::fs::read_link"
            | "std::fs::canonicalize"
            | "std::fs::File::open"
            | "fs::read"
            | "fs::read_to_string"
            | "fs::read_link"
            | "fs::canonicalize"
            | "fs::File::open"
            | "File::open"
    )
}

fn process_or_network_call(path: &str) -> bool {
    matches!(
        path,
        "std::process::Command::new"
            | "process::Command::new"
            | "Command::new"
            | "std::net::TcpStream::connect"
            | "net::TcpStream::connect"
            | "TcpStream::connect"
            | "std::net::UdpSocket::bind"
            | "net::UdpSocket::bind"
            | "UdpSocket::bind"
            | "std::os::unix::net::UnixStream::connect"
            | "UnixStream::connect"
    )
}

fn cargo_derived_environment(name: &str) -> bool {
    name.starts_with("CARGO_")
        || matches!(
            name,
            "OUT_DIR"
                | "TARGET"
                | "HOST"
                | "PROFILE"
                | "OPT_LEVEL"
                | "DEBUG"
                | "NUM_JOBS"
                | "RUSTC"
                | "RUSTDOC"
        )
}

fn package_generated_outputs(
    package: &PackageInventory,
) -> Result<Vec<String>, WorkspaceTestError> {
    let mut outputs = BTreeSet::new();
    for target in &package.declared_targets {
        if compile_time_kind(target).is_some() {
            continue;
        }
        let mut sources = BTreeSet::from([target.source.clone()]);
        sources.extend(
            target
                .declared_path_sources
                .iter()
                .filter(|source| source.path.extension() == Some(OsStr::new("rs")))
                .map(|source| source.path.clone()),
        );
        for source in sources {
            let document = fs::read_to_string(&source).map_err(|error| {
                non_cacheable(format!(
                    "read Cargo package {:?} source {} for OUT_DIR authority: {error}",
                    package.name,
                    source.display(),
                ))
            })?;
            let syntax = syn::parse_file(&document).map_err(|error| {
                non_cacheable(format!(
                    "parse Cargo package {:?} source {} for OUT_DIR authority: {error}",
                    package.name,
                    source.display(),
                ))
            })?;
            let mut visitor = GeneratedOutputVisitor {
                source: &source,
                outputs: &mut outputs,
                error: None,
            };
            visitor.visit_file(&syntax);
            if let Some(error) = visitor.error {
                return Err(error);
            }
        }
    }
    Ok(outputs.into_iter().collect())
}

struct GeneratedOutputVisitor<'a> {
    source: &'a Path,
    outputs: &'a mut BTreeSet<String>,
    error: Option<WorkspaceTestError>,
}

impl<'ast> Visit<'ast> for GeneratedOutputVisitor<'_> {
    fn visit_macro(&mut self, invocation: &'ast syn::Macro) {
        if self.error.is_some() {
            return;
        }
        let is_file_macro =
            super::source_reach::FileMacroKind::from_path(&invocation.path).is_some();
        if is_file_macro {
            match out_dir_include(invocation.tokens.clone()) {
                Ok(Some(output)) => {
                    self.outputs.insert(output);
                }
                Ok(None) => {}
                Err(detail) => {
                    self.error = Some(non_cacheable(format!(
                        "Cargo source {} has unbounded OUT_DIR input: {detail}",
                        self.source.display(),
                    )));
                }
            }
        }
        syn::visit::visit_macro(self, invocation);
    }
}

fn out_dir_include(tokens: proc_macro2::TokenStream) -> Result<Option<String>, String> {
    let Ok(expression) = syn::parse2::<Expr>(tokens) else {
        return Ok(None);
    };
    let Expr::Macro(concat) = expression else {
        return Ok(None);
    };
    if !concat.mac.path.is_ident("concat") {
        return Ok(None);
    }
    let parser = Punctuated::<Expr, syn::Token![,]>::parse_terminated;
    let arguments = parser
        .parse2(concat.mac.tokens)
        .map_err(|error| error.to_string())?;
    let [Expr::Macro(environment), Expr::Lit(suffix)] =
        arguments.iter().collect::<Vec<_>>().as_slice()
    else {
        return Err("expected concat!(env!(\"OUT_DIR\"), \"/literal\")".to_owned());
    };
    if !environment.mac.path.is_ident("env") {
        return Err("expected env!(\"OUT_DIR\") as the first concat argument".to_owned());
    }
    let variable = syn::parse2::<syn::LitStr>(environment.mac.tokens.clone())
        .map_err(|error| error.to_string())?;
    let Lit::Str(suffix) = &suffix.lit else {
        return Err("expected a literal OUT_DIR suffix".to_owned());
    };
    if variable.value() != "OUT_DIR" {
        return Ok(None);
    }
    let suffix = suffix.value();
    let relative = suffix.strip_prefix('/').ok_or_else(|| {
        "OUT_DIR suffix must begin with one slash before a relative path".to_owned()
    })?;
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "OUT_DIR suffix {suffix:?} is not a normalized relative file"
        ));
    }
    Ok(Some(relative.replace(std::path::MAIN_SEPARATOR, "/")))
}

fn admit_declared_file(
    package: &PackageInventory,
    selected_root: &Path,
    declared: &str,
    selected_paths: &BTreeSet<&str>,
) -> Result<String, WorkspaceTestError> {
    let relative = Path::new(declared);
    if relative.is_absolute() {
        return Err(non_cacheable(format!(
            "Cargo package {:?} declares external compile-time path {declared:?}",
            package.name,
        )));
    }
    let candidate = package.root.join(relative);
    let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
        non_cacheable(format!(
            "Cargo package {:?} declared compile-time path {} is unavailable: {error}",
            package.name,
            candidate.display(),
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(non_cacheable(format!(
            "Cargo package {:?} declared compile-time path {} is not a non-symlink regular file",
            package.name,
            candidate.display(),
        )));
    }
    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        non_cacheable(format!(
            "canonicalize Cargo package {:?} compile-time path {}: {error}",
            package.name,
            candidate.display(),
        ))
    })?;
    let relative = canonical.strip_prefix(selected_root).map_err(|_| {
        non_cacheable(format!(
            "Cargo package {:?} compile-time path {} escapes selected root {}",
            package.name,
            canonical.display(),
            selected_root.display(),
        ))
    })?;
    let relative = path_text(relative)?;
    if !selected_paths.contains(relative.as_str()) {
        return Err(non_cacheable(format!(
            "Cargo package {:?} compile-time path {relative:?} is absent from selected-tree authority",
            package.name,
        )));
    }
    Ok(relative)
}

fn validate_environment_name(package: &str, name: &str) -> Result<(), WorkspaceTestError> {
    if name.is_empty() || name.as_bytes().contains(&b'=') || name.as_bytes().contains(&0) {
        return Err(non_cacheable(format!(
            "Cargo package {package:?} declares invalid compile-time environment name {name:?}"
        )));
    }
    Ok(())
}

fn non_cacheable(detail: impl Into<String>) -> WorkspaceTestError {
    WorkspaceTestError::new(format!(
        "Cargo shard is non-cacheable because executable compile-time authority is incomplete: {}",
        detail.into(),
    ))
}
