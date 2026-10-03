//! The standard test result (spec §3.5): the LAST line of a test script's
//! stdout.
//!
//! ```json
//! {"name":"<test node name>","id":"<test node id>","overall_success":true,
//!  "normal":{"total":50,"pass":50,"err":0},"longtail":{"total":0,"pass":0,"err":0},
//!  "failure":{"total":0,"pass":0,"err":0},
//!  "details":[{"name":"t1","bucket":"normal","pass":true,"msg":""}]}
//! ```
//! Legacy `ITER_RESULT pass=X fail=Y total=Z` is still accepted (→ `normal`).
//! Exit code 0 = pass, 1 = fail, anything else = could not run.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub pass: u64,
    #[serde(default)]
    pub err: u64,
}

impl Bucket {
    /// every test in the bucket passed and none errored
    pub fn green(&self) -> bool {
        self.err == 0 && self.pass >= self.total
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detail {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub pass: bool,
    #[serde(default)]
    pub msg: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub overall_success: bool,
    #[serde(default)]
    pub normal: Bucket,
    #[serde(default)]
    pub longtail: Bucket,
    #[serde(default)]
    pub failure: Bucket,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<Detail>,
}

/// What the script's last line may hold before defaults are filled in.
#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    overall_success: Option<bool>,
    #[serde(default)]
    normal: Option<Bucket>,
    #[serde(default)]
    longtail: Option<Bucket>,
    #[serde(default)]
    failure: Option<Bucket>,
    #[serde(default)]
    details: Option<Vec<Detail>>,
}

impl TestResult {
    pub fn totals(&self) -> Bucket {
        Bucket {
            total: self.normal.total + self.longtail.total + self.failure.total,
            pass: self.normal.pass + self.longtail.pass + self.failure.pass,
            err: self.normal.err + self.longtail.err + self.failure.err,
        }
    }

    /// A result for a script that printed none: one test, pass = exit 0.
    pub fn from_exit(name: &str, id: &str, exit_code: Option<i32>) -> TestResult {
        let pass = exit_code == Some(0);
        TestResult {
            name: name.to_string(),
            id: id.to_string(),
            overall_success: pass,
            normal: Bucket { total: 1, pass: pass as u64, err: (!pass) as u64 },
            ..Default::default()
        }
    }

    /// Fill a missing name / id (legacy lines carry neither).
    pub fn with_identity(mut self, name: &str, id: &str) -> TestResult {
        if self.name.is_empty() {
            self.name = name.to_string();
        }
        if self.id.is_empty() {
            self.id = id.to_string();
        }
        self
    }

    /// The one-line JSON form a script prints.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Parse the standard result from a script's stdout: the last non-empty line
/// as JSON (an object with at least one of `overall_success` / `normal` /
/// `longtail` / `failure`), else the last `ITER_RESULT pass= fail= total=`
/// line. A missing `overall_success` is derived (every bucket green and at
/// least one test). None = no result line.
pub fn parse_last_line(stdout: &str) -> Option<TestResult> {
    if let Some(last) = stdout.lines().rev().map(str::trim).find(|l| !l.is_empty()) {
        if last.starts_with('{') {
            if let Ok(raw) = serde_json::from_str::<Raw>(last) {
                if raw.overall_success.is_some() || raw.normal.is_some() || raw.longtail.is_some() || raw.failure.is_some() {
                    let mut r = TestResult {
                        name: raw.name.unwrap_or_default(),
                        id: raw.id.unwrap_or_default(),
                        overall_success: false,
                        normal: raw.normal.unwrap_or_default(),
                        longtail: raw.longtail.unwrap_or_default(),
                        failure: raw.failure.unwrap_or_default(),
                        details: raw.details.unwrap_or_default(),
                    };
                    r.overall_success = raw.overall_success.unwrap_or_else(|| {
                        r.totals().total > 0 && r.normal.green() && r.longtail.green() && r.failure.green()
                    });
                    return Some(r);
                }
            }
        }
    }
    parse_legacy(stdout)
}

fn parse_legacy(stdout: &str) -> Option<TestResult> {
    let line = stdout.lines().rev().map(str::trim).find(|l| l.starts_with("ITER_RESULT"))?;
    let (mut pass, mut fail, mut total) = (None, None, None);
    for tok in line.split_whitespace().skip(1) {
        if let Some((k, v)) = tok.split_once('=') {
            let n = v.trim_end_matches(',').parse::<u64>().ok();
            match k {
                "pass" => pass = n,
                "fail" => fail = n,
                "total" => total = n,
                _ => {}
            }
        }
    }
    let pass = pass?;
    let total = match (total, fail) {
        (Some(t), _) => t,
        (None, Some(f)) => pass + f,
        (None, None) => return None,
    };
    let err = fail.unwrap_or(total.saturating_sub(pass));
    let normal = Bucket { total, pass, err };
    Some(TestResult { overall_success: total > 0 && normal.green(), normal, ..Default::default() })
}

/// The verdict of one script run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Pass,
    Fail,
    CouldNotRun,
}

/// Combine the exit code with the parsed result: exit other than 0/1 (or a
/// signal, `None`) = could not run; exit 1 = fail; exit 0 = pass unless the
/// result line says `overall_success: false`.
pub fn combine(result: Option<&TestResult>, exit_code: Option<i32>) -> Outcome {
    match exit_code {
        Some(0) => match result {
            Some(r) if !r.overall_success => Outcome::Fail,
            _ => Outcome::Pass,
        },
        Some(1) => Outcome::Fail,
        _ => Outcome::CouldNotRun,
    }
}

/// Parse + combine in one go: the result (synthesised from the exit code
/// when the script printed none, name / id filled in) and the outcome.
pub fn evaluate(stdout: &str, exit_code: Option<i32>, name: &str, id: &str) -> (TestResult, Outcome) {
    let parsed = parse_last_line(stdout);
    let outcome = combine(parsed.as_ref(), exit_code);
    let mut r = match parsed {
        Some(r) => r.with_identity(name, id),
        None => TestResult::from_exit(name, id, exit_code),
    };
    if outcome != Outcome::Pass {
        r.overall_success = false;
    }
    (r, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STD: &str = r#"{"name":"db","id":"6a1c","overall_success":true,"normal":{"total":50,"pass":50,"err":0},"longtail":{"total":0,"pass":0,"err":0},"failure":{"total":0,"pass":0,"err":0},"details":[{"name":"t1","bucket":"normal","pass":true,"msg":""}]}"#;

    #[test]
    fn standard_line_parses() {
        let r = parse_last_line(&format!("building...\nok\n{}\n\n", STD)).unwrap();
        assert_eq!(r.name, "db");
        assert_eq!(r.id, "6a1c");
        assert!(r.overall_success);
        assert_eq!(r.normal, Bucket { total: 50, pass: 50, err: 0 });
        assert_eq!(r.details.len(), 1);
        assert_eq!(r.details[0].bucket, "normal");
    }

    #[test]
    fn round_trip_line() {
        let r = parse_last_line(STD).unwrap();
        assert_eq!(parse_last_line(&r.to_line()).unwrap(), r);
    }

    #[test]
    fn details_optional_and_overall_derived() {
        let r = parse_last_line(r#"{"normal":{"total":3,"pass":2,"err":1}}"#).unwrap();
        assert!(!r.overall_success);
        assert!(r.details.is_empty());
        let r = parse_last_line(r#"{"normal":{"total":3,"pass":3,"err":0}}"#).unwrap();
        assert!(r.overall_success);
    }

    #[test]
    fn explicit_overall_wins() {
        let r = parse_last_line(r#"{"overall_success":false,"normal":{"total":1,"pass":1,"err":0}}"#).unwrap();
        assert!(!r.overall_success);
    }

    #[test]
    fn legacy_iter_result() {
        let r = parse_last_line("x\nITER_RESULT pass=12 fail=2 total=14\n").unwrap();
        assert_eq!(r.normal, Bucket { total: 14, pass: 12, err: 2 });
        assert!(!r.overall_success);
        let r = parse_last_line("ITER_RESULT pass=3 fail=0 total=3").unwrap();
        assert!(r.overall_success);
        let r = parse_last_line("ITER_RESULT pass=3 fail=1").unwrap();
        assert_eq!(r.normal.total, 4);
    }

    #[test]
    fn legacy_line_need_not_be_last() {
        let r = parse_last_line("ITER_RESULT pass=1 fail=0 total=1\ncleanup done\n").unwrap();
        assert!(r.overall_success);
    }

    #[test]
    fn malformed_lines_are_none() {
        assert!(parse_last_line("").is_none());
        assert!(parse_last_line("all good\n").is_none());
        assert!(parse_last_line("{not json").is_none());
        assert!(parse_last_line(r#"{"foo":1}"#).is_none());
        assert!(parse_last_line("[1,2]").is_none());
        assert!(parse_last_line("ITER_RESULT pass=x total=3").is_none());
        assert!(parse_last_line(r#"{"normal":{"total":"many"}}"#).is_none());
    }

    #[test]
    fn exit_code_combination() {
        let green = parse_last_line(STD).unwrap();
        let red = parse_last_line("ITER_RESULT pass=1 fail=1 total=2").unwrap();
        assert_eq!(combine(Some(&green), Some(0)), Outcome::Pass);
        assert_eq!(combine(Some(&red), Some(0)), Outcome::Fail);
        assert_eq!(combine(Some(&green), Some(1)), Outcome::Fail);
        assert_eq!(combine(None, Some(0)), Outcome::Pass);
        assert_eq!(combine(None, Some(2)), Outcome::CouldNotRun);
        assert_eq!(combine(Some(&green), None), Outcome::CouldNotRun);
        assert_eq!(combine(None, Some(-1)), Outcome::CouldNotRun);
    }

    #[test]
    fn evaluate_synthesises_and_fills_identity() {
        let (r, o) = evaluate("no result here", Some(1), "db tests", "id-1");
        assert_eq!(o, Outcome::Fail);
        assert_eq!((r.name.as_str(), r.id.as_str()), ("db tests", "id-1"));
        assert_eq!(r.normal, Bucket { total: 1, pass: 0, err: 1 });
        let (r, o) = evaluate("ITER_RESULT pass=2 fail=0 total=2", Some(0), "n", "i");
        assert_eq!(o, Outcome::Pass);
        assert_eq!(r.name, "n");
        assert!(r.overall_success);
        let (r, o) = evaluate(STD, Some(3), "other", "x");
        assert_eq!(o, Outcome::CouldNotRun);
        assert_eq!(r.name, "db", "a name in the line is kept");
        assert!(!r.overall_success);
    }
}
