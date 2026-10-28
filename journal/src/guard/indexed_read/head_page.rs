//! Joined, bounded pages over current task and trust authority.

use rusqlite::{Row, params};

use super::{FactReadBudget, GuardError, JournalHead, JournalRead, TaskHeads, read_metrics};

/// Cursor identity shared by every page in one current-head projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CurrentHeadWitness {
    /// Immutable journal cursor observed by the enclosing read transaction.
    pub cursor: Option<wire::Seq>,
}

/// One ordered bounded page of complete current task-head bundles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskHeadPage {
    /// Complete bundles decoded within this call's row and byte budgets.
    pub rows: Vec<TaskHeads>,
    /// Last returned stamp when more current task authority remains.
    pub continuation: Option<wire::TaskStamp>,
    /// Cursor identity of the projection this page observed.
    pub witness: CurrentHeadWitness,
}

/// One ordered bounded page of current trust heads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustHeadPage {
    /// Trust heads decoded within this call's row and byte budgets.
    pub rows: Vec<JournalHead<wire::TrustHead>>,
    /// Last returned digest when more current trust authority remains.
    pub continuation: Option<wire::Digest>,
    /// Cursor identity of the projection this page observed.
    pub witness: CurrentHeadWitness,
}

impl JournalRead<'_> {
    /// Read one joined page of task statement, workflow, claim, and lease heads.
    pub fn task_head_page(
        &self,
        after: Option<&wire::TaskStamp>,
        budget: FactReadBudget,
    ) -> Result<TaskHeadPage, GuardError> {
        self.task_head_page_with(
            TASK_PAGE_SQL,
            params![after.map(wire::TaskStamp::as_str), query_limit(budget)?],
            budget,
        )
    }

    /// Read one joined page of current task heads for an exact project.
    pub fn task_project_head_page(
        &self,
        project: &str,
        after: Option<&wire::TaskStamp>,
        budget: FactReadBudget,
    ) -> Result<TaskHeadPage, GuardError> {
        self.task_head_page_with(
            TASK_PROJECT_PAGE_SQL,
            params![
                project,
                after.map(wire::TaskStamp::as_str),
                query_limit(budget)?
            ],
            budget,
        )
    }

    /// Current direct neighbors in either dependency direction, including settled work.
    pub fn task_neighbor_head_page(
        &self,
        task: &wire::TaskStamp,
        dependencies: &[wire::TaskStamp],
        after: Option<&wire::TaskStamp>,
        budget: FactReadBudget,
    ) -> Result<TaskHeadPage, GuardError> {
        let dependencies = serde_json::to_string(dependencies)
            .map_err(|error| GuardError::Invalid(error.to_string()))?;
        self.task_head_page_with(
            TASK_NEIGHBOR_PAGE_SQL,
            params![
                task.as_str(),
                dependencies,
                after.map(wire::TaskStamp::as_str),
                query_limit(budget)?
            ],
            budget,
        )
    }

    fn task_head_page_with<P: rusqlite::Params>(
        &self,
        sql: &str,
        parameters: P,
        budget: FactReadBudget,
    ) -> Result<TaskHeadPage, GuardError> {
        read_metrics::observe_query();
        let mut statement = self.connection.prepare(sql).map_err(GuardError::Store)?;
        let mut rows = statement.query(parameters).map_err(GuardError::Store)?;
        let mut tasks = Vec::with_capacity(budget.rows());
        let mut decoded = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            let indexed = row.get::<_, String>(0).map_err(GuardError::Store)?;
            read_metrics::observe_row();
            if tasks.len() == budget.rows() {
                continuation = tasks
                    .last()
                    .map(|task: &TaskHeads| task.statement.body.born.clone());
                break;
            }
            let row_bytes = task_row_bytes(row, &indexed)?;
            if decoded.saturating_add(row_bytes) > budget.decoded_bytes() {
                if tasks.is_empty() {
                    return Err(GuardError::Invalid(format!(
                        "task head key {indexed:?} has {row_bytes} body bytes, exceeding this call's {}-byte decoded budget",
                        budget.decoded_bytes(),
                    )));
                }
                continuation = tasks.last().map(|task| task.statement.body.born.clone());
                break;
            }
            let task = decode_task_row(row, &indexed, self.cursor)?;
            read_metrics::observe_decode(row_bytes);
            decoded = decoded.saturating_add(row_bytes);
            tasks.push(task);
        }
        Ok(TaskHeadPage {
            rows: tasks,
            continuation,
            witness: CurrentHeadWitness {
                cursor: self.cursor,
            },
        })
    }

    /// Read one joined page of current trust heads in digest order.
    pub fn trust_head_page(
        &self,
        after: Option<&wire::Digest>,
        budget: FactReadBudget,
    ) -> Result<TrustHeadPage, GuardError> {
        let query_limit = budget
            .rows()
            .checked_add(1)
            .expect("bounded trust page probe");
        let query_limit = i64::try_from(query_limit)
            .map_err(|_| GuardError::Invalid("trust head page limit exceeds SQLite".to_owned()))?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(TRUST_PAGE_SQL)
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![after.map(wire::Digest::as_str), query_limit])
            .map_err(GuardError::Store)?;
        let mut heads = Vec::with_capacity(budget.rows());
        let mut decoded = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            let indexed = row.get::<_, String>(0).map_err(GuardError::Store)?;
            read_metrics::observe_row();
            if heads.len() == budget.rows() {
                continuation = heads
                    .last()
                    .map(|head: &JournalHead<wire::TrustHead>| head.body.digest.clone());
                break;
            }
            let length = joined_length(row, 2, "trust head", &indexed)?;
            if decoded.saturating_add(length) > budget.decoded_bytes() {
                if heads.is_empty() {
                    return Err(GuardError::Invalid(format!(
                        "trust head key {indexed:?} has {length} body bytes, exceeding this call's {}-byte decoded budget",
                        budget.decoded_bytes(),
                    )));
                }
                continuation = heads.last().map(|head| head.body.digest.clone());
                break;
            }
            let position = row.get::<_, i64>(1).map_err(GuardError::Store)?;
            let head: JournalHead<wire::TrustHead> = decode_joined_head(
                row,
                position,
                3,
                "trust_heads",
                "digest",
                &indexed,
                self.cursor,
            )?;
            let digest = indexed.parse::<wire::Digest>().map_err(|error| {
                GuardError::Invalid(format!("trust head key {indexed:?} is invalid: {error}"))
            })?;
            if head.body.digest != digest {
                return Err(GuardError::Invalid(format!(
                    "trust head indexed as {digest} carries body digest {}",
                    head.body.digest
                )));
            }
            read_metrics::observe_decode(length);
            decoded = decoded.saturating_add(length);
            heads.push(head);
        }
        Ok(TrustHeadPage {
            rows: heads,
            continuation,
            witness: CurrentHeadWitness {
                cursor: self.cursor,
            },
        })
    }
}

fn query_limit(budget: FactReadBudget) -> Result<i64, GuardError> {
    let limit = budget
        .rows()
        .checked_add(1)
        .expect("bounded task page probe");
    i64::try_from(limit)
        .map_err(|_| GuardError::Invalid("task head page limit exceeds SQLite".to_owned()))
}

const TRUST_PAGE_SQL: &str = "SELECT digest, head_seq, length(body), body FROM trust_heads \
    WHERE digest > COALESCE(?1, '') ORDER BY digest LIMIT ?2";

const TASK_PAGE_SQL: &str = "WITH task_keys(stamp) AS (\
    SELECT stamp FROM (SELECT stamp FROM task_heads \
        WHERE stamp > COALESCE(?1, '') ORDER BY stamp LIMIT ?2) \
    UNION SELECT stamp FROM (SELECT stamp FROM workflow_heads \
        WHERE stamp > COALESCE(?1, '') ORDER BY stamp LIMIT ?2) \
    UNION SELECT stamp FROM (SELECT stamp FROM claim_heads \
        WHERE stamp > COALESCE(?1, '') ORDER BY stamp LIMIT ?2) \
    UNION SELECT stamp FROM (SELECT stamp FROM lease_heads \
        WHERE stamp > COALESCE(?1, '') ORDER BY stamp LIMIT ?2)\
) SELECT keys.stamp, statement.head_seq, length(statement.body), statement.body, \
         workflow.head_seq, length(workflow.body), workflow.body, \
         claim.head_seq, length(claim.body), claim.body, \
         lease.head_seq, length(lease.body), lease.body \
  FROM task_keys AS keys \
  LEFT JOIN task_heads AS statement ON statement.stamp = keys.stamp \
  LEFT JOIN workflow_heads AS workflow ON workflow.stamp = keys.stamp \
  LEFT JOIN claim_heads AS claim ON claim.stamp = keys.stamp \
  LEFT JOIN lease_heads AS lease ON lease.stamp = keys.stamp \
 ORDER BY keys.stamp LIMIT ?2";

const TASK_PROJECT_PAGE_SQL: &str = "WITH task_keys(stamp) AS (\
    SELECT stamp FROM task_heads \
     WHERE json_extract(CAST(body AS TEXT), '$.project') = ?1 \
       AND stamp > COALESCE(?2, '') ORDER BY stamp LIMIT ?3\
) SELECT keys.stamp, statement.head_seq, length(statement.body), statement.body, \
         workflow.head_seq, length(workflow.body), workflow.body, \
         claim.head_seq, length(claim.body), claim.body, \
         lease.head_seq, length(lease.body), lease.body \
  FROM task_keys AS keys \
  LEFT JOIN task_heads AS statement ON statement.stamp = keys.stamp \
  LEFT JOIN workflow_heads AS workflow ON workflow.stamp = keys.stamp \
  LEFT JOIN claim_heads AS claim ON claim.stamp = keys.stamp \
  LEFT JOIN lease_heads AS lease ON lease.stamp = keys.stamp \
 ORDER BY keys.stamp LIMIT ?3";

const TASK_NEIGHBOR_PAGE_SQL: &str = "WITH task_keys(stamp) AS (\
    SELECT stamp FROM task_heads \
     WHERE stamp > COALESCE(?3, '') AND (\
       stamp IN (SELECT value FROM json_each(?2)) OR \
       EXISTS (SELECT 1 FROM json_each(CAST(body AS TEXT), '$.dependencies') WHERE value = ?1)) \
     ORDER BY stamp LIMIT ?4\
) SELECT keys.stamp, statement.head_seq, length(statement.body), statement.body, \
         workflow.head_seq, length(workflow.body), workflow.body, \
         claim.head_seq, length(claim.body), claim.body, \
         lease.head_seq, length(lease.body), lease.body \
  FROM task_keys AS keys \
  LEFT JOIN task_heads AS statement ON statement.stamp = keys.stamp \
  LEFT JOIN workflow_heads AS workflow ON workflow.stamp = keys.stamp \
  LEFT JOIN claim_heads AS claim ON claim.stamp = keys.stamp \
  LEFT JOIN lease_heads AS lease ON lease.stamp = keys.stamp \
 ORDER BY keys.stamp LIMIT ?4";

#[cfg(test)]
pub(crate) fn verify_task_page_plan(connection: &rusqlite::Connection) -> Result<(), String> {
    let mut trust_plans = Vec::new();
    for after in [None, Some("spice.test.digest")] {
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {TRUST_PAGE_SQL}"))
            .map_err(|error| error.to_string())?;
        let plan = statement
            .query_map(params![after, 3_i64], |row| row.get::<_, String>(3))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        trust_plans.push(plan);
    }
    let trust_seeks = trust_plans.iter().all(|plan| {
        plan.iter().any(|step| step.contains("(digest>?)"))
            && !plan.iter().any(|step| step.contains("SCAN trust_heads "))
    });
    let mut statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {TASK_PAGE_SQL}"))
        .map_err(|error| error.to_string())?;
    let plan = statement
        .query_map(params![Option::<String>::None, 3_i64], |row| {
            row.get::<_, String>(3)
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let seeks = plan
        .iter()
        .filter(|step| step.contains("(stamp>?)"))
        .count();
    let full_scan = plan.iter().any(|step| {
        ["task_heads", "workflow_heads", "claim_heads", "lease_heads"]
            .iter()
            .any(|table| step.contains(&format!("SCAN {table} ")))
    });
    let mut project_statement = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {TASK_PROJECT_PAGE_SQL}"))
        .map_err(|error| error.to_string())?;
    let project_plan = project_statement
        .query_map(
            params!["journal.fixture", Option::<String>::None, 3_i64],
            |row| row.get::<_, String>(3),
        )
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let project_seek = project_plan
        .iter()
        .any(|step| step.contains("task_heads_project_stamp"));
    (trust_seeks && seeks == 4 && !full_scan && project_seek)
        .then_some(())
        .ok_or_else(|| {
            format!(
                "expected bounded trust, global-task, and exact-project seeks, found trust={trust_plans:?} global={plan:?} project={project_plan:?}"
            )
        })
}

fn task_row_bytes(row: &Row<'_>, indexed: &str) -> Result<usize, GuardError> {
    [2, 5, 8, 11]
        .into_iter()
        .map(|column| {
            row.get::<_, Option<i64>>(column)
                .map_err(GuardError::Store)?
                .map(|length| joined_length_value(length, "task head", indexed))
                .transpose()
        })
        .try_fold(0usize, |total, length| {
            total
                .checked_add(length?.unwrap_or_default())
                .ok_or_else(|| {
                    GuardError::Invalid(format!("task head key {indexed:?} byte count overflowed"))
                })
        })
}

fn decode_task_row(
    row: &Row<'_>,
    indexed: &str,
    cursor: Option<wire::Seq>,
) -> Result<TaskHeads, GuardError> {
    let stamp = indexed.parse::<wire::TaskStamp>().map_err(|error| {
        GuardError::Invalid(format!("task head key {indexed:?} is invalid: {error}"))
    })?;
    let statement = optional_head::<wire::TaskHead>(row, 1, 3, "task_heads", indexed, cursor)?;
    let workflow =
        optional_head::<wire::WorkflowHead>(row, 4, 6, "workflow_heads", indexed, cursor)?;
    let claim = optional_head::<wire::ClaimHead>(row, 7, 9, "claim_heads", indexed, cursor)?;
    let lease = optional_head::<wire::LeaseHead>(row, 10, 12, "lease_heads", indexed, cursor)?;
    let loose = [
        workflow.as_ref().map(|_| "workflow"),
        claim.as_ref().map(|_| "claim"),
        lease.as_ref().map(|_| "lease"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let statement: JournalHead<wire::TaskHead> = statement.ok_or_else(|| {
        GuardError::Invalid(format!(
            "task {stamp} has loose {} head(s) without a statement head",
            loose.join(", ")
        ))
    })?;
    let workflow = workflow.ok_or_else(|| {
        GuardError::Invalid(format!(
            "task {stamp} has a statement head but no workflow head"
        ))
    })?;
    if statement.body.born != stamp {
        return Err(GuardError::Invalid(format!(
            "task head indexed as {stamp} carries body stamp {}",
            statement.body.born
        )));
    }
    if statement.body.revision != statement.position {
        return Err(GuardError::Invalid(format!(
            "task {stamp} statement head revision disagrees with head_seq"
        )));
    }
    if let Some(claim) = &claim
        && claim.body.fact != claim.position
    {
        return Err(GuardError::Invalid(format!(
            "task {stamp} claim head fact disagrees with head_seq"
        )));
    }
    Ok(TaskHeads {
        statement,
        workflow,
        claim,
        lease,
    })
}

fn optional_head<T>(
    row: &Row<'_>,
    position_column: usize,
    body_column: usize,
    table: &'static str,
    key: &str,
    cursor: Option<wire::Seq>,
) -> Result<Option<JournalHead<T>>, GuardError>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let position = row
        .get::<_, Option<i64>>(position_column)
        .map_err(GuardError::Store)?;
    position
        .map(|position| decode_joined_head(row, position, body_column, table, "stamp", key, cursor))
        .transpose()
}

fn decode_joined_head<T>(
    row: &Row<'_>,
    position: i64,
    body_column: usize,
    table: &'static str,
    key_column: &'static str,
    key: &str,
    cursor: Option<wire::Seq>,
) -> Result<JournalHead<T>, GuardError>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let bytes = row
        .get::<_, Vec<u8>>(body_column)
        .map_err(GuardError::Store)?;
    let head = super::decode_head(position, &bytes).map_err(|error| {
        let detail = match error {
            GuardError::Invalid(detail) => detail,
            other => other.to_string(),
        };
        GuardError::Invalid(format!("{table} {key_column} {key:?} is invalid: {detail}"))
    })?;
    if cursor.is_none_or(|cursor| head.position > cursor) {
        return Err(GuardError::Invalid(format!(
            "{table} {key_column} {key:?} head position {} is beyond cursor {:?}",
            head.position,
            cursor.map(wire::Seq::get)
        )));
    }
    Ok(head)
}

fn joined_length(
    row: &Row<'_>,
    column: usize,
    class: &str,
    key: &str,
) -> Result<usize, GuardError> {
    let length = row.get::<_, i64>(column).map_err(GuardError::Store)?;
    joined_length_value(length, class, key)
}

fn joined_length_value(length: i64, class: &str, key: &str) -> Result<usize, GuardError> {
    usize::try_from(length)
        .map_err(|_| GuardError::Invalid(format!("{class} key {key:?} has invalid body length")))
}
