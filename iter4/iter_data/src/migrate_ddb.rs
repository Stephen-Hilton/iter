//! `--migrate-from dynamodb`: copy every iter3 DynamoDB table into this
//! backend (normally ArangoDB), row for row with pk/sk/version preserved, then
//! the seq counters with their values. DynamoDB is only read: the source is
//! opened with `DdbBackend::new_readonly`, which cannot create tables.
//! Idempotent: rows already in the target are skipped unless --migrate-overwrite.

use crate::ddb::DdbBackend;
use crate::storage::{Storage, StorageError};
use std::collections::HashSet;

#[derive(Debug, Default)]
pub struct TableReport {
    pub table: String,
    pub source: usize,
    pub written: usize,
    pub skipped: usize,
    pub target_after: usize,
}

pub async fn copy_all(
    src: &dyn Storage,
    dst: &dyn Storage,
    tables: &[String],
    dry_run: bool,
    overwrite: bool,
) -> Result<(Vec<TableReport>, usize), StorageError> {
    let mut reports = Vec::new();
    for t in tables {
        let rows = src.scan_keyed(t).await?;
        let existing: HashSet<(String, String)> =
            dst.scan_keyed(t).await?.into_iter().map(|r| (r.pk, r.sk)).collect();
        let mut rep = TableReport { table: t.clone(), source: rows.len(), ..Default::default() };
        for row in &rows {
            if !overwrite && existing.contains(&(row.pk.clone(), row.sk.clone())) {
                rep.skipped += 1;
                continue;
            }
            if !dry_run {
                dst.put_keyed(t, row).await?;
            }
            rep.written += 1;
        }
        rep.target_after = if dry_run { existing.len() } else { dst.scan_keyed(t).await?.len() };
        reports.push(rep);
    }
    // seq counters: carry each value over; never move a target counter backwards
    let mut seqs = 0;
    let dst_seq: std::collections::HashMap<(String, String), u64> = dst
        .all_versions()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|v| ((v.projectname, v.table), v.seq))
        .collect();
    for v in src.all_versions().await? {
        let cur = dst_seq.get(&(v.projectname.clone(), v.table.clone())).copied().unwrap_or(0);
        if cur >= v.seq && !overwrite {
            continue;
        }
        if !dry_run {
            dst.set_seq(&v).await?;
        }
        seqs += 1;
    }
    Ok((reports, seqs))
}

pub async fn run_cli(
    dst: &dyn Storage,
    from: &str,
    prefix: &str,
    region: &str,
    tables: &str,
    dry_run: bool,
    overwrite: bool,
) {
    if from != "dynamodb" {
        eprintln!("--migrate-from supports only 'dynamodb' (got '{from}')");
        std::process::exit(2);
    }
    if !prefix.starts_with("iter3") {
        // guardrail carried from iter3: never read outside our own namespace
        eprintln!("refusing source prefix '{prefix}' — must start with 'iter3'");
        std::process::exit(2);
    }
    let region = if !region.is_empty() {
        region.to_string()
    } else {
        std::env::var("AWS_DEFAULT_REGION").unwrap_or_else(|_| "us-west-2".into())
    };
    let src = match DdbBackend::new_readonly(&region, prefix).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[migrate-from] dynamodb connect failed: {e}");
            std::process::exit(1);
        }
    };
    let wanted: Vec<String> = if tables.trim().is_empty() {
        iter_core::TABLES.iter().filter(|t| **t != "versions").map(|t| t.to_string()).collect()
    } else {
        tables.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
    };
    println!(
        "[migrate-from] {}dynamodb {region} {prefix}* -> {} ({} tables)",
        if dry_run { "DRY RUN: " } else { "" },
        dst.backend_name(),
        wanted.len()
    );
    match copy_all(&src, dst, &wanted, dry_run, overwrite).await {
        Ok((reports, seqs)) => {
            let mut short = false;
            println!("  {:<22} {:>8} {:>8} {:>8} {:>8}", "table", "source", "written", "skipped", "target");
            for r in &reports {
                println!("  {:<22} {:>8} {:>8} {:>8} {:>8}", r.table, r.source, r.written, r.skipped, r.target_after);
                if !dry_run && r.target_after < r.source {
                    short = true;
                }
            }
            println!("  seq counters set: {seqs}");
            if short {
                eprintln!("[migrate-from] FAILED: a target table holds fewer rows than its source");
                std::process::exit(1);
            }
            println!("[migrate-from] {}", if dry_run { "dry run complete (nothing written)" } else { "complete: every target table holds at least its source's rows" });
        }
        Err(e) => {
            eprintln!("[migrate-from] FAILED: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn copies_rows_keys_versions_and_seq_idempotently() {
        // the copy only needs the Storage trait: two throwaway databases stand in for DynamoDB and the target
        let (src, sdb) = crate::test_db::store().await;
        let (dst, ddb) = crate::test_db::store().await;
        src.put_versioned("workitem", "p", "w1", &json!({"id": "w1", "version": 1}), 0).await.unwrap();
        src.put_versioned("workitem", "p", "w1", &json!({"id": "w1", "version": 2}), 1).await.unwrap();
        src.put("lock", "p", "{topdir}/x/", &json!({"path": "{topdir}/x/", "expires": "z", "workid": "w1"})).await.unwrap();
        src.bump_seq("p", "workitem").await.unwrap();
        src.bump_seq("p", "workitem").await.unwrap();
        let tables = vec!["workitem".to_string(), "lock".to_string()];
        let (rep, seqs) = copy_all(&src, &dst, &tables, false, false).await.unwrap();
        assert_eq!(rep[0].written, 1);
        assert_eq!(seqs, 1);
        // the native version survived, so the next versioned write expects 2
        dst.put_versioned("workitem", "p", "w1", &json!({"id": "w1", "version": 3}), 2).await.unwrap();
        assert_eq!(dst.get_versions("p").await.unwrap()[0].seq, 2);
        // second run: everything skipped
        let (rep2, seqs2) = copy_all(&src, &dst, &tables, false, false).await.unwrap();
        assert_eq!((rep2[0].written, rep2[0].skipped, seqs2), (0, 1, 0));
        crate::test_db::drop(&sdb).await;
        crate::test_db::drop(&ddb).await;
    }
}
