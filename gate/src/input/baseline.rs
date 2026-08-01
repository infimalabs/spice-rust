/// One loaded lower-is-better floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Floor {
    name: String,
    value: u64,
}

impl Floor {
    /// Stable current gate name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Maximum debt admitted by the tracked state.
    #[must_use]
    pub const fn value(&self) -> u64 {
        self.value
    }
}

/// Complete, canonical `baseline.toml` state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    floors: Vec<Floor>,
}

impl Baseline {
    fn parse(path: &Path, text: &str, gate_set: &GateSet) -> Result<Self, BaselineError> {
        let table = parse_baseline_table(path, text)?;
        validate_baseline_version(path, &table)?;
        let rows = baseline_rows(path, &table)?;
        let floors = parse_floor_rows(path, rows, gate_set)?;
        require_all_floors(path, &floors, gate_set)?;
        validate_mutation_floor(path, &floors)?;

        let baseline = Self { floors };
        let canonical = baseline.render();
        if canonical != text {
            return Err(BaselineError::NonCanonical {
                path: path.to_path_buf(),
                field: "document".to_owned(),
                canonical,
            });
        }
        Ok(baseline)
    }

    /// Every floor in current gate-name order.
    #[must_use]
    pub fn floors(&self) -> &[Floor] {
        &self.floors
    }

    /// One floor by current gate name.
    #[must_use]
    pub fn floor(&self, name: &str) -> Option<u64> {
        self.floors
            .iter()
            .find(|floor| floor.name == name)
            .map(|floor| floor.value)
    }

    /// The one admitted baseline spelling.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = format!("version = {BASELINE_VERSION}\n");
        for floor in &self.floors {
            let _ = write!(
                rendered,
                "\n[[floor]]\nname = \"{}\"\nvalue = {}\n",
                floor.name, floor.value
            );
        }
        rendered
    }

    /// Identity of the complete versioned state.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::compute(Domain::GateBaseline, self.render().as_bytes())
    }

    fn ratchet(&self, gate_set: &GateSet) -> Ratchet {
        let mut text = String::new();
        for declaration in gate_set
            .floors
            .iter()
            .filter(|floor| matches!(floor.kind, FloorKind::Measurement))
        {
            let value = self
                .floor(declaration.name())
                .expect("a loaded baseline carries every declared floor");
            let _ = writeln!(text, "{} {value}", declaration.name());
        }
        Ratchet::parse(&text).expect("validated baseline names render one ratchet spelling")
    }

    fn mutation_floor(&self) -> SurvivorRate {
        SurvivorRate::new(
            self.floor("rust.mutation-survivors")
                .expect("a loaded baseline carries the mutation floor"),
            1,
        )
        .expect("baseline admission restricts the mutation floor to zero or one")
    }
}

fn validate_mutation_floor(path: &Path, floors: &[Floor]) -> Result<(), BaselineError> {
    let floor = floors
        .iter()
        .find(|floor| floor.name == "rust.mutation-survivors")
        .expect("the complete current baseline carries the mutation floor");
    if floor.value > 1 {
        return Err(BaselineError::MutationRate {
            path: path.to_path_buf(),
            field: "floor[rust.mutation-survivors].value".to_owned(),
            value: floor.value,
        });
    }
    Ok(())
}

fn parse_baseline_table(path: &Path, text: &str) -> Result<toml::Table, BaselineError> {
    let table = text
        .parse::<toml::Table>()
        .map_err(|source| BaselineError::Toml {
            path: path.to_path_buf(),
            source,
        })?;
    reject_unknown_keys(path, "", &table, &["version", "floor"]).map_err(BaselineError::Field)?;
    Ok(table)
}

fn validate_baseline_version(path: &Path, table: &toml::Table) -> Result<(), BaselineError> {
    let version = required(path, table, "version").map_err(BaselineError::Field)?;
    let version = version
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| {
            BaselineError::Field(FieldError::type_mismatch(
                path,
                "version",
                "a non-negative integer",
            ))
        })?;
    if version != BASELINE_VERSION {
        return Err(BaselineError::UnsupportedVersion {
            path: path.to_path_buf(),
            field: "version".to_owned(),
            version,
        });
    }
    Ok(())
}

fn baseline_rows<'a>(
    path: &Path,
    table: &'a toml::Table,
) -> Result<&'a [toml::Value], BaselineError> {
    required(path, table, "floor")
        .map_err(BaselineError::Field)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| {
            BaselineError::Field(FieldError::type_mismatch(
                path,
                "floor",
                "an array of floor tables",
            ))
        })
}

fn parse_floor_rows(
    path: &Path,
    rows: &[toml::Value],
    gate_set: &GateSet,
) -> Result<Vec<Floor>, BaselineError> {
    let declared: BTreeSet<&str> = gate_set.floors.iter().map(|floor| floor.name()).collect();
    let mut floors = Vec::with_capacity(rows.len());
    let mut seen = BTreeSet::new();
    let mut previous: Option<String> = None;
    for (index, row) in rows.iter().enumerate() {
        let (floor, name_field) = parse_floor_row(path, index, row)?;
        validate_floor_name(
            path,
            &name_field,
            floor.name(),
            &declared,
            &mut seen,
            previous.as_deref(),
        )?;
        previous = Some(floor.name.clone());
        floors.push(floor);
    }
    Ok(floors)
}

fn parse_floor_row(
    path: &Path,
    index: usize,
    row: &toml::Value,
) -> Result<(Floor, String), BaselineError> {
    let row_field = format!("floor[{index}]");
    let row = row.as_table().ok_or_else(|| {
        BaselineError::Field(FieldError::type_mismatch(path, &row_field, "a floor table"))
    })?;
    reject_unknown_keys(path, &row_field, row, &["name", "value"]).map_err(BaselineError::Field)?;

    let name_field = format!("{row_field}.name");
    let name = required(path, row, &name_field)
        .map_err(BaselineError::Field)?
        .as_str()
        .ok_or_else(|| {
            BaselineError::Field(FieldError::type_mismatch(path, &name_field, "a string"))
        })?;
    let value_field = format!("{row_field}.value");
    let value = required(path, row, &value_field)
        .map_err(BaselineError::Field)?
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| {
            BaselineError::Field(FieldError::type_mismatch(
                path,
                &value_field,
                "a non-negative integer",
            ))
        })?;
    Ok((
        Floor {
            name: name.to_owned(),
            value,
        },
        name_field,
    ))
}

fn validate_floor_name(
    path: &Path,
    field: &str,
    name: &str,
    declared: &BTreeSet<&str>,
    seen: &mut BTreeSet<String>,
    previous: Option<&str>,
) -> Result<(), BaselineError> {
    if !seen.insert(name.to_owned()) {
        return Err(BaselineError::Duplicate {
            path: path.to_path_buf(),
            field: field.to_owned(),
            name: name.to_owned(),
        });
    }
    if RETIRED_FLOORS.contains(&name) {
        return Err(BaselineError::Retired {
            path: path.to_path_buf(),
            field: field.to_owned(),
            name: name.to_owned(),
        });
    }
    if previous.is_some_and(|earlier| earlier > name) {
        return Err(BaselineError::Unsorted {
            path: path.to_path_buf(),
            field: field.to_owned(),
            previous: previous
                .expect("the comparison proved a previous floor")
                .to_owned(),
            name: name.to_owned(),
        });
    }
    if !declared.contains(name) {
        return Err(BaselineError::Unknown {
            path: path.to_path_buf(),
            field: field.to_owned(),
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn require_all_floors(
    path: &Path,
    floors: &[Floor],
    gate_set: &GateSet,
) -> Result<(), BaselineError> {
    let present: BTreeSet<&str> = floors.iter().map(|floor| floor.name()).collect();
    if let Some(missing) = gate_set
        .floors
        .iter()
        .find(|floor| !present.contains(floor.name()))
    {
        return Err(BaselineError::Missing {
            path: path.to_path_buf(),
            field: "floor".to_owned(),
            name: missing.name.clone(),
        });
    }
    Ok(())
}
