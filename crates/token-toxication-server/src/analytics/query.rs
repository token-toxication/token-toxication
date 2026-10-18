use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row, params, params_from_iter, types::Value};

use super::*;

// Only these bounded metadata columns leave SQLite. Neither errors nor request
// summaries are returned. Old/unknown classifications are never inferred.
const SCOPED: &str = r#"
WITH scoped AS (
 SELECT l.*,
   (unixepoch(l.created_at) - ?5) / ?6 AS bucket,
   CASE WHEN outcome IN ('success','client_disconnect','server_shutdown',
     'replay_missing_item','turn_state','invalid_request','rate_limited','overload',
     'incomplete','authorization','transport','upstream_error','client_error','other')
     THEN outcome ELSE 'unknown' END AS result,
   CASE WHEN transport IN ('http','http_sse','websocket') THEN transport ELSE 'unknown' END AS channel,
   CASE WHEN json_valid(request_summary) THEN
     json_extract(request_summary, '$.usageComplete') = 1 ELSE 0 END AS usage_ok
 FROM request_logs l
 WHERE created_at >= ?1 AND created_at < ?2
   AND (?3 IS NULL OR model = ?3)
   AND (?4 IS NULL OR provider_account_id = ?4
        OR (?4 = 'unassigned' AND provider_account_id IS NULL))
)
"#;

const AGGREGATES: &str = r#"
 COUNT(*),
 COALESCE(SUM(result='success'),0),
 COALESCE(SUM(result NOT IN ('success','client_disconnect','server_shutdown','unknown')),0),
 COALESCE(SUM(result='client_disconnect'),0),
 COALESCE(SUM(result='server_shutdown'),0),
 COALESCE(SUM(result='unknown'),0),
 COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
 COALESCE(SUM(CASE WHEN usage_ok AND cached_input_tokens BETWEEN 0 AND input_tokens THEN input_tokens ELSE 0 END),0),
 COALESCE(SUM(CASE WHEN usage_ok AND cached_input_tokens BETWEEN 0 AND input_tokens THEN cached_input_tokens ELSE 0 END),0),
 COALESCE(SUM(COALESCE(usage_ok,0) AND cached_input_tokens BETWEEN 0 AND input_tokens),0),
 COALESCE(SUM(COALESCE(usage_ok,0)),0),
 COALESCE(SUM(attempt_index>0),0), COALESCE(SUM(is_overload_retry=1),0)
"#;

fn stats(row: &Row<'_>) -> rusqlite::Result<AnalyticsStats> {
    Ok(AnalyticsStats {
        attempts: row.get(1)?,
        success: row.get(2)?,
        failures: row.get(3)?,
        client_disconnect: row.get(4)?,
        server_shutdown: row.get(5)?,
        unknown: row.get(6)?,
        input_tokens: row.get(7)?,
        output_tokens: row.get(8)?,
        classified_input_tokens: row.get(9)?,
        cached_input_tokens: row.get(10)?,
        cache_samples: row.get(11)?,
        complete_usage_samples: row.get(12)?,
        retry_attempts: row.get(13)?,
        overload_retries: row.get(14)?,
        ..Default::default()
    })
}

pub(super) fn history(
    conn: &mut Connection,
    query: &AnalyticsQuery,
    now: DateTime<Utc>,
) -> rusqlite::Result<AnalyticsHistory> {
    let (seconds, count) = query.range.dimensions();
    let current = now.timestamp().div_euclid(seconds) * seconds;
    let start =
        DateTime::from_timestamp(current - (count - 1) * seconds, 0).expect("analytics window");
    let binds = vec![
        Value::Text(start.to_rfc3339()),
        Value::Text(now.to_rfc3339()),
        query.model.clone().map_or(Value::Null, Value::Text),
        query.account_id.clone().map_or(Value::Null, Value::Text),
        Value::Integer(start.timestamp()),
        Value::Integer(seconds),
    ];
    // One consistent WAL snapshot across counts, percentiles and rankings.
    let tx = conn.transaction()?;
    let mut history = AnalyticsHistory {
        range: query.range,
        model: query.model.clone(),
        account_id: query.account_id.clone(),
        rank_by: query.rank_by,
        window_started_at: start,
        window_ended_at: now,
        generated_at: now,
        bucket_duration_seconds: seconds as u64,
        summary: Default::default(),
        buckets: (0..count)
            .map(|index| AnalyticsBucket {
                started_at: DateTime::from_timestamp(start.timestamp() + index * seconds, 0)
                    .expect("bucket time"),
                partial: index == count - 1,
                stats: Default::default(),
            })
            .collect(),
        outcomes: vec![],
        transports: vec![],
        statuses: vec![],
        models: vec![],
        accounts: vec![],
    };
    let sql = format!(
        "{SCOPED} SELECT -1, {AGGREGATES} FROM scoped UNION ALL SELECT bucket, {AGGREGATES} FROM scoped GROUP BY bucket"
    );
    let mut stmt = tx.prepare(&sql)?;
    for row in stmt.query_map(params_from_iter(&binds), |row| {
        Ok((row.get::<_, i64>(0)?, stats(row)?))
    })? {
        let (index, stats) = row?;
        *stats_at(&mut history, index)? = stats;
    }
    drop(stmt);

    // Exact nearest-rank selection is linear-time and much cheaper than sorting
    // eight copies of every value with SQLite window functions. Bound memory
    // explicitly; over-limit queries fail rather than silently sampling.
    const MAX_MEASURED_ATTEMPTS: usize = 500_000;
    let mut measurements: Vec<[Vec<f64>; 4]> = (0..=count)
        .map(|_| std::array::from_fn(|_| Vec::new()))
        .collect();
    let sql = format!(
        "{SCOPED} SELECT bucket,ttft_ms,latency_ms,queue_wait_ms,otps FROM scoped WHERE result='success'"
    );
    let mut stmt = tx.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(&binds), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            [
                row.get::<_, Option<f64>>(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ],
        ))
    })?;
    for (observed, row) in rows.enumerate() {
        if observed >= MAX_MEASURED_ATTEMPTS {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let (index, values) = row?;
        for (metric, value) in values.into_iter().enumerate() {
            if let Some(value) = value
                .filter(|value| value.is_finite() && *value >= 0.0 && (metric != 3 || *value > 0.0))
            {
                measurements[0][metric].push(value);
                measurements[index as usize + 1][metric].push(value);
            }
        }
    }
    drop(stmt);
    for (index, values) in measurements.into_iter().enumerate() {
        let stats = stats_at(&mut history, index as i64 - 1)?;
        let [ttft, latency, queue, otps] = values.map(percentiles);
        stats.ttft = ttft;
        stats.latency = latency;
        stats.queue = queue;
        stats.otps = otps;
    }
    for (column, target) in [
        ("result", &mut history.outcomes),
        ("channel", &mut history.transports),
        ("status_code", &mut history.statuses),
    ] {
        let sql = format!(
            "{SCOPED} SELECT CAST({column} AS TEXT),COUNT(*) FROM scoped GROUP BY {column} ORDER BY COUNT(*) DESC,{column}"
        );
        *target = tx
            .prepare(&sql)?
            .query_map(params_from_iter(&binds), |row| {
                Ok(AnalyticsCount {
                    name: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
    }
    history.models = ranking(
        &tx,
        &binds,
        AnalyticsDimension::Model,
        query.rank_by,
        &history.summary,
    )?;
    history.accounts = ranking(
        &tx,
        &binds,
        AnalyticsDimension::Account,
        query.rank_by,
        &history.summary,
    )?;
    tx.commit()?;
    Ok(history)
}

fn percentiles(mut values: Vec<f64>) -> AnalyticsPercentiles {
    let n = values.len();
    if n == 0 {
        return Default::default();
    }
    let p50 = *values.select_nth_unstable_by((n - 1) / 2, f64::total_cmp).1;
    let p95 = *values
        .select_nth_unstable_by((n * 95).div_ceil(100) - 1, f64::total_cmp)
        .1;
    AnalyticsPercentiles {
        samples: n as u64,
        p50: Some(p50),
        p95: Some(p95),
    }
}

fn stats_at(history: &mut AnalyticsHistory, index: i64) -> rusqlite::Result<&mut AnalyticsStats> {
    if index == -1 {
        return Ok(&mut history.summary);
    }
    history
        .buckets
        .get_mut(index as usize)
        .map(|bucket| &mut bucket.stats)
        .ok_or(rusqlite::Error::InvalidQuery)
}

fn ranking(
    conn: &Connection,
    binds: &[Value],
    dimension: AnalyticsDimension,
    rank: AnalyticsRankBy,
    summary: &AnalyticsStats,
) -> rusqlite::Result<Vec<AnalyticsGroup>> {
    let (id, name, deleted, join) = match dimension {
        AnalyticsDimension::Model => ("s.model", "s.model", "0", ""),
        AnalyticsDimension::Account => (
            "s.provider_account_id",
            "a.name",
            "s.provider_account_id IS NOT NULL AND a.id IS NULL",
            "LEFT JOIN provider_accounts a ON a.id=s.provider_account_id",
        ),
    };
    let order = match rank {
        AnalyticsRankBy::Attempts => "attempts",
        AnalyticsRankBy::Tokens => "tokens",
        AnalyticsRankBy::Failures => "failures",
    };
    let sql = format!(
        r#"{SCOPED} SELECT {id}, {name}, {deleted}, COUNT(*) attempts,
      SUM(result NOT IN ('success','client_disconnect','server_shutdown','unknown')) failures,
      SUM(input_tokens+output_tokens) tokens FROM scoped s {join}
      GROUP BY {id} ORDER BY {order} DESC,{id} LIMIT 10"#
    );
    let mut groups = conn
        .prepare(&sql)?
        .query_map(params_from_iter(binds), |row| {
            Ok(AnalyticsGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                deleted: row.get(2)?,
                other: false,
                attempts: row.get(3)?,
                failures: row.get(4)?,
                tokens: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let remainder = AnalyticsGroup {
        id: None,
        name: None,
        deleted: false,
        other: true,
        attempts: summary.attempts - groups.iter().map(|g| g.attempts).sum::<u64>(),
        failures: summary.failures - groups.iter().map(|g| g.failures).sum::<u64>(),
        tokens: summary.input_tokens + summary.output_tokens
            - groups.iter().map(|g| g.tokens).sum::<u64>(),
    };
    if remainder.attempts > 0 {
        groups.push(remainder);
    }
    Ok(groups)
}

pub(super) fn options(
    conn: &Connection,
    query: &AnalyticsOptionsQuery,
    now: DateTime<Utc>,
) -> rusqlite::Result<AnalyticsOptions> {
    let (seconds, count) = query.range.dimensions();
    let start = DateTime::from_timestamp(
        now.timestamp().div_euclid(seconds) * seconds - (count - 1) * seconds,
        0,
    )
    .expect("options window");
    let (id, name, deleted, join) = match query.dimension {
        AnalyticsDimension::Model => ("l.model", "l.model", "0", ""),
        AnalyticsDimension::Account => (
            "l.provider_account_id",
            "a.name",
            "l.provider_account_id IS NOT NULL AND a.id IS NULL",
            "LEFT JOIN provider_accounts a ON a.id=l.provider_account_id",
        ),
    };
    let sql = format!(
        r#"SELECT {id}, {name}, {deleted} FROM request_logs l {join}
        WHERE l.created_at>=?1 AND l.created_at<?2
          AND (?3='' OR instr(lower(COALESCE({id},'')||' '||COALESCE({name},'')),lower(?3))>0)
        GROUP BY {id} ORDER BY {id} LIMIT 51 OFFSET ?4"#
    );
    let mut data = conn
        .prepare(&sql)?
        .query_map(
            params![
                start.to_rfc3339(),
                now.to_rfc3339(),
                query.search,
                query.page * 50
            ],
            |row| {
                Ok(AnalyticsGroup {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    deleted: row.get(2)?,
                    other: false,
                    attempts: 0,
                    failures: 0,
                    tokens: 0,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = data.len() > 50;
    data.truncate(50);
    Ok(AnalyticsOptions { data, has_more })
}

#[cfg(test)]
mod tests;
