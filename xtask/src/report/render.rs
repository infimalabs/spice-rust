//! Pinned editorial renderer and one local, network-disabled print boundary.

use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::Command;

use super::publication::{Publication, VERSION, derived_json};
use super::{Result, hash_file};

mod charts;
mod pages;

const FONT_FILES: [&str; 4] = [
    "lmroman10-regular.otf",
    "lmroman10-italic.otf",
    "lmsans10-regular.otf",
    "lmsans10-bold.otf",
];
const STYLE: &str = ::std::include_str!("render/style.css");
const PRINT_SOURCE: &str = ::std::include_str!("print.mjs");
const GUST_LICENSE: &str = ::std::include_str!("GUST-FONT-LICENSE.txt");

#[derive(Serialize)]
struct Manifest {
    version: u32,
    title: &'static str,
    calendar_zone: &'static str,
    horizon_day: String,
    publication_schema: u32,
    offline: bool,
    private_originals_included: bool,
    assets: BTreeMap<String, String>,
    print_boundary: serde_json::Value,
}

pub(super) fn render(
    private: &Path,
    public: &Path,
    fonts: &Path,
    license: &Path,
    output: &mut dyn Write,
) -> Result<()> {
    let data = private.join("publication-data.json");
    let publication = pinned_publication(private, &data)?;
    let private = fs::canonicalize(private)?;
    disjoint(&private, public)?;
    write_publication(&publication, &data, public, fonts, license)?;
    let print = print_admission(public, &private)?;
    derived_json(
        &public.join("manifest.json"),
        &Manifest {
            version: VERSION,
            title: "Making work land",
            calendar_zone: "UTC",
            horizon_day: publication.horizon_day,
            publication_schema: publication.version,
            offline: true,
            private_originals_included: false,
            assets: asset_digests(public)?,
            print_boundary: print,
        },
    )?;
    writeln!(
        output,
        "offline field guide: {}; private originals remain in {}",
        public.display(),
        private.display()
    )?;
    Ok(())
}

/// Read the publication only when it is the exact one analysis pinned.
fn pinned_publication(private: &Path, data: &Path) -> Result<Publication> {
    let analysis: serde_json::Value =
        serde_json::from_reader(File::open(private.join("analysis-manifest.json"))?)?;
    if analysis
        .get("publication_sha256")
        .and_then(serde_json::Value::as_str)
        != Some(hash_file(data)?.as_str())
    {
        return Err("publication data disagrees with its pinned analysis manifest".into());
    }
    let publication: Publication = serde_json::from_reader(File::open(data)?)?;
    if publication.version != VERSION {
        return Err("unsupported publication schema; use its owning report consumer".into());
    }
    Ok(publication)
}

/// Refuse a publication tree that would sit inside, or contain, private evidence.
fn disjoint(private: &Path, public: &Path) -> Result<()> {
    let parent = fs::canonicalize(
        public
            .parent()
            .ok_or("publication needs a parent directory")?,
    )?;
    let name = public
        .file_name()
        .ok_or("publication needs a directory name")?;
    if parent.starts_with(private) || private.starts_with(parent.join(name)) {
        return Err("private evidence and publication trees must be disjoint".into());
    }
    Ok(())
}

fn write_publication(
    publication: &Publication,
    data: &Path,
    public: &Path,
    fonts: &Path,
    license: &Path,
) -> Result<()> {
    fs::create_dir(public)?;
    fs::create_dir(public.join("assets"))?;
    for font in FONT_FILES {
        fs::copy(fonts.join(font), public.join("assets").join(font))?;
    }
    fs::copy(license, public.join("assets/LPPL.txt"))?;
    fs::write(public.join("assets/GUST-FONT-LICENSE.txt"), GUST_LICENSE)?;
    fs::write(public.join("assets/style.css"), STYLE)?;
    fs::copy(data, public.join("data.json"))?;
    let document = pages::document(publication, public)?;
    super::privacy::scan(&document)?;
    fs::write(public.join("index.html"), document)?;
    fs::write(public.join("tables.html"), pages::tables(publication))?;
    fs::write(public.join("tasks.csv"), task_csv(publication))?;
    fs::write(
        public.join("measurements.csv"),
        measurements_csv(publication),
    )?;
    Ok(())
}

/// Run the pinned offline print bridge and return its public receipt.
fn print_admission(public: &Path, private: &Path) -> Result<serde_json::Value> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/report/print.mjs");
    if fs::read_to_string(&script)? != PRINT_SOURCE {
        return Err("print bridge changed after consumer compilation".into());
    }
    let result = Command::new("node")
        .arg(script)
        .arg(fs::canonicalize(public)?)
        .arg(private)
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "offline print admission failed: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&result.stdout)?)
}

fn asset_digests(public: &Path) -> Result<BTreeMap<String, String>> {
    let mut assets = BTreeMap::new();
    for entry in fs::read_dir(public)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "non-UTF8 publication member")?;
            assets.insert(name, hash_file(&entry.path())?);
        }
    }
    for entry in fs::read_dir(public.join("assets"))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF8 publication asset")?;
        assets.insert(format!("assets/{name}"), hash_file(&entry.path())?);
    }
    Ok(assets)
}

fn task_csv(publication: &Publication) -> String {
    let mut csv = "task,domain,selected,admitted_day,closed_day,closure_route,phase\n".to_owned();
    for task in &publication.tasks {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            task.id,
            task.domain,
            task.selected,
            task.admitted_day,
            task.closed_day.as_deref().unwrap_or(""),
            task.closure_route,
            task.phase
        ));
    }
    csv
}

fn measurements_csv(publication: &Publication) -> String {
    let mut csv =
        "quantity,population,unit,denominator,observed,missing,censored,label,value\n".to_owned();
    for quantity in &publication.quantities {
        for row in &quantity.values {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{}\n",
                quantity.id,
                csv_field(&quantity.population),
                quantity.unit,
                quantity.denominator,
                quantity.observed,
                quantity.missing,
                quantity.censored,
                csv_field(&row.label),
                row.value
            ));
        }
    }
    csv
}

fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

pub(super) fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(super) fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}
