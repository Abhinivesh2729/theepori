//! Usage, cost and productivity (PRD §19). Every number says where it came
//! from; estimates are labelled; missing data stays missing.

use rusqlite::Connection;
use tp_proto::{Budget, BudgetStatus, UsageBucket, UsageSummary};

use crate::{Result, store};

/// Founder minutes assumed per manual task of each category, used for the
/// hours-saved estimate (disclosed in the summary).
pub const BASELINE_MINUTES: &[(&str, f64)] = &[
    ("incident", 120.0),
    ("security", 90.0),
    ("ci", 45.0),
    ("bug", 60.0),
    ("deps", 30.0),
    ("maintenance", 45.0),
    ("feature", 180.0),
];
/// Founder minutes counted per approval decision.
pub const APPROVAL_MINUTES: f64 = 2.0;

fn days_ago(days: u32) -> String {
    let t = time::OffsetDateTime::now_utc() - time::Duration::days(i64::from(days));
    let f = time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].000Z");
    t.format(&f).unwrap_or_default()
}

/// Start of the current UTC day / month, RFC 3339.
pub fn period_start(period: &str) -> String {
    let now = time::OffsetDateTime::now_utc();
    if period == "month" {
        format!(
            "{:04}-{:02}-01T00:00:00.000Z",
            now.year(),
            u8::from(now.month())
        )
    } else {
        format!(
            "{:04}-{:02}-{:02}T00:00:00.000Z",
            now.year(),
            u8::from(now.month()),
            now.day()
        )
    }
}

pub fn budget_status(c: &Connection) -> Result<Vec<BudgetStatus>> {
    store::list_budgets(c)?
        .into_iter()
        .map(|b| {
            let spent = store::spent_since(c, &b.scope, &period_start(&b.period))?;
            Ok(BudgetStatus {
                exceeded: spent >= b.limit_usd,
                spent_usd: spent,
                budget: b,
            })
        })
        .collect()
}

/// First exceeded budget that applies to `project`, if any.
pub fn exceeded_for(c: &Connection, project: &str) -> Result<Option<Budget>> {
    Ok(budget_status(c)?
        .into_iter()
        .find(|s| s.exceeded && (s.budget.scope == "global" || s.budget.scope == project))
        .map(|s| s.budget))
}

fn buckets(c: &Connection, key_sql: &str, since: &str) -> Result<Vec<UsageBucket>> {
    let mut st = c.prepare(&format!(
        "SELECT {key_sql} AS k, COUNT(*), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0)
         FROM usage WHERE ts >= ?1 GROUP BY k ORDER BY 5 DESC"
    ))?;
    let rows = st.query_map([since], |r| {
        Ok(UsageBucket {
            key: r
                .get::<_, Option<String>>(0)?
                .unwrap_or_else(|| "(none)".into()),
            runs: r.get::<_, i64>(1)? as u64,
            input_tokens: r.get::<_, i64>(2)? as u64,
            output_tokens: r.get::<_, i64>(3)? as u64,
            cost_usd: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn count(c: &Connection, sql: &str, since: &str) -> Result<u64> {
    Ok(c.query_row(sql, [since], |r| r.get::<_, i64>(0))? as u64)
}

pub fn summary(c: &Connection, days: u32) -> Result<UsageSummary> {
    let since = days_ago(days.max(1));
    let (input, output, cost): (i64, i64, f64) = c.query_row(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0), COALESCE(SUM(cost_usd),0) FROM usage WHERE ts >= ?1",
        [&since],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let runs = count(
        c,
        "SELECT COUNT(*) FROM runs WHERE started_at >= ?1",
        &since,
    )?;
    let runs_failed = count(
        c,
        "SELECT COUNT(*) FROM runs WHERE started_at >= ?1 AND state IN ('failed','interrupted')",
        &since,
    )?;
    let retries = count(
        c,
        "SELECT COUNT(*) FROM runs WHERE started_at >= ?1 AND kind = 'repair'",
        &since,
    )?;
    let tasks_done = count(
        c,
        "SELECT COUNT(*) FROM tasks WHERE updated_at >= ?1 AND state = 'done'",
        &since,
    )?;
    let tasks_blocked = count(
        c,
        "SELECT COUNT(*) FROM tasks WHERE updated_at >= ?1 AND state IN ('blocked','escalated')",
        &since,
    )?;
    let approvals = count(
        c,
        "SELECT COUNT(*) FROM approvals WHERE decided_at >= ?1",
        &since,
    )?;
    let interventions = count(
        c,
        "SELECT COUNT(*) FROM interventions WHERE ts >= ?1",
        &since,
    )?;
    let (pass, total): (i64, i64) = c.query_row(
        "SELECT COALESCE(SUM(status = 'passed'),0), COUNT(*) FROM verifications WHERE created_at >= ?1",
        [&since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // PR cycle: task creation -> merged (from the audit index).
    let avg_cycle: Option<f64> = c.query_row(
        "SELECT AVG((julianday(a.ts) - julianday(t.created_at)) * 24.0) FROM audit_index a
         JOIN tasks t ON ('T' || t.n) = a.task WHERE a.action = 'pr.merged' AND a.ts >= ?1",
        [&since],
        |r| r.get(0),
    )?;
    // Hours saved: baseline per finished task minus measured founder time.
    let mut st = c.prepare("SELECT category, COUNT(*) FROM tasks WHERE updated_at >= ?1 AND state = 'done' GROUP BY category")?;
    let done: Vec<(String, i64)> = st
        .query_map([&since], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let baseline: f64 = done
        .iter()
        .map(|(cat, n)| {
            BASELINE_MINUTES
                .iter()
                .find(|(k, _)| k == cat)
                .map_or(60.0, |(_, m)| *m)
                * *n as f64
        })
        .sum();
    let human: f64 = c.query_row(
        "SELECT COALESCE(SUM(minutes),0) FROM interventions WHERE ts >= ?1",
        [&since],
        |r| r.get(0),
    )?;
    let human = human + approvals as f64 * APPROVAL_MINUTES;
    Ok(UsageSummary {
        days,
        runs,
        runs_failed,
        retries,
        input_tokens: input as u64,
        output_tokens: output as u64,
        reported_cost_usd: cost,
        cost_basis: "sum of Claude Code reported total_cost_usd (list-price estimate on a subscription; not a bill)".into(),
        by_project: buckets(c, "project", &since)?,
        by_model: buckets(c, "model", &since)?,
        by_day: buckets(c, "substr(ts, 1, 10)", &since)?,
        tasks_done,
        tasks_blocked,
        approvals,
        interventions,
        verification_pass_rate: (total > 0).then(|| pass as f64 / total as f64),
        avg_pr_cycle_hours: avg_cycle,
        hours_saved: (tasks_done > 0).then(|| ((baseline - human) / 60.0).max(0.0)),
        hours_saved_method: format!(
            "Estimate: per finished task a fixed manual baseline (minutes: {}) minus measured founder time \
             (logged interventions + {APPROVAL_MINUTES} min per approval). Not measured against real manual work.",
            BASELINE_MINUTES.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
        ),
        budgets: budget_status(c)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{RunOutcome, testutil::*};
    use tp_proto::{RunKind, RunState};

    #[test]
    fn summary_and_budgets() {
        let (_d, c) = conn_with_project("app");
        task(&c, "app", "x", None);
        let r = store::insert_run(
            &c,
            store::NewRun {
                task_n: Some(1),
                project: Some("app"),
                kind: RunKind::Implement,
                runtime: "claude",
                model: Some("m"),
            },
        )
        .unwrap();
        let o = RunOutcome {
            state: Some(RunState::Succeeded),
            cost_usd: Some(1.5),
            input_tokens: Some(10),
            output_tokens: Some(5),
            ..Default::default()
        };
        store::finish_run(&c, r, &o).unwrap();
        store::record_usage(
            &c,
            Some(r),
            Some("app"),
            Some(1),
            RunKind::Implement,
            Some("m"),
            &o,
        )
        .unwrap();
        store::set_budget(
            &c,
            &Budget {
                scope: "app".into(),
                period: "day".into(),
                limit_usd: 1.0,
            },
        )
        .unwrap();
        let s = summary(&c, 7).unwrap();
        assert_eq!(s.runs, 1);
        assert!((s.reported_cost_usd - 1.5).abs() < 1e-9);
        assert_eq!(s.by_model[0].key, "m");
        assert!(s.budgets[0].exceeded);
        assert!(s.hours_saved.is_none(), "no finished tasks -> no estimate");
        assert!(exceeded_for(&c, "app").unwrap().is_some());
        assert!(exceeded_for(&c, "other").unwrap().is_none());
    }
}
