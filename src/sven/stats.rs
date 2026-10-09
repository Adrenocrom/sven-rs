//! Lifetime run statistics, persisted in `<data_dir>/statistics.json`.
//!
//! The counters are updated after every run and the file rewritten
//! immediately, so it always mirrors the sessions so far and survives
//! restarts. A missing file starts at zero; a file that cannot be read
//! or parsed is reported on stderr and also starts at zero — statistics
//! must never keep the agent from running.
//!
//! Besides the overall counters, runs are folded into per-model and
//! per-host maps, so switching between servers and models keeps
//! comparable numbers for each. Each host bucket additionally breaks
//! its runs down per model, so model use per host stays visible even
//! when several models share one server.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::sven::agent::{format_duration, format_tokens};
use crate::sven::chat_history::TokenUsage;
use crate::sven::skills::expand_tilde;

pub const STATS_FILE: &str = "statistics.json";

/// How a run ended. `Error` covers request failures (connection refused,
/// HTTP error status); `RoundCap` is the `MAX_TOOL_ROUNDS` bail-out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    /// The model produced a final answer.
    Finished,
    /// A request failed — no answer was produced.
    Error,
    /// Stopped after the maximum number of tool rounds.
    RoundCap,
}

/// What `Agent::run_rounds` reports back for the statistics: how many LLM
/// rounds and tool calls the run took, and how it ended.
#[derive(Debug)]
pub struct RunOutcome {
    pub rounds: u64,
    pub tool_calls: u64,
    pub status: RunStatus,
}

/// Counters for one bucket — the whole history, or one model / one host.
/// Fields missing from a file written by an older version default to
/// 0/None.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(default)]
pub struct RunCounters {
    pub runs: u64,
    /// Runs that ended in a request error.
    pub errors: u64,
    /// Runs stopped by the tool-round cap.
    pub round_caps: u64,
    /// LLM round trips across all runs.
    pub rounds: u64,
    pub tool_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Wall time of all runs combined, in seconds.
    pub total_duration_secs: f64,
    /// RFC 3339 timestamps of the first and most recent recorded run.
    pub first_run: Option<String>,
    pub last_run: Option<String>,
}

impl RunCounters {
    /// Fold one finished run into the counters.
    fn add_run(&mut self, usage: &TokenUsage, outcome: &RunOutcome, duration: Duration) {
        let now = Local::now().to_rfc3339();
        self.runs += 1;
        match outcome.status {
            RunStatus::Finished => {}
            RunStatus::Error => self.errors += 1,
            RunStatus::RoundCap => self.round_caps += 1,
        }
        self.rounds += outcome.rounds;
        self.tool_calls += outcome.tool_calls;
        self.prompt_tokens += usage.prompt_tokens;
        self.completion_tokens += usage.completion_tokens;
        self.total_duration_secs += duration.as_secs_f64();
        if self.first_run.is_none() {
            self.first_run = Some(now.clone());
        }
        self.last_run = Some(now);
    }

    /// One compact line for the per-model / per-host sections of the
    /// summary: runs, tokens and wall time — the overall section above
    /// already shows the detailed breakdown.
    fn summary_line(&self) -> String {
        let total = Duration::from_secs_f64(self.total_duration_secs);
        format!(
            "{} runs, {} in, {} out, {}",
            self.runs,
            format_tokens(self.prompt_tokens),
            format_tokens(self.completion_tokens),
            format_duration(total)
        )
    }
}

/// Counters for one host: the host's totals plus the same counters
/// broken down per model, so model use per host stays visible. The
/// totals are flattened, so files written before the per-model
/// breakdown existed (plain `RunCounters` per host) still load — the
/// model sections then start empty.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(default)]
pub struct HostCounters {
    #[serde(flatten)]
    pub total: RunCounters,
    pub per_model: BTreeMap<String, RunCounters>,
}

impl HostCounters {
    /// Fold one finished run into the host totals and the bucket of
    /// the model it ran against.
    fn add_run(&mut self, model: &str, usage: &TokenUsage, outcome: &RunOutcome, duration: Duration) {
        self.total.add_run(usage, outcome, duration);
        self.per_model
            .entry(model.to_string())
            .or_default()
            .add_run(usage, outcome, duration);
    }
}

/// Lifetime counters — the JSON mirror of `statistics.json`. The
/// top-level fields are the totals across everything; `per_model` and
/// `per_host` hold the same counters keyed by model name and host URL.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(default)]
pub struct Statistics {
    #[serde(flatten)]
    pub overall: RunCounters,
    pub per_model: BTreeMap<String, RunCounters>,
    pub per_host: BTreeMap<String, HostCounters>,
}

impl Statistics {
    /// Fold one finished run into the overall counters and the buckets
    /// of the model and host it ran against.
    fn add_run(
        &mut self,
        model: &str,
        host: &str,
        usage: &TokenUsage,
        outcome: &RunOutcome,
        duration: Duration,
    ) {
        self.overall.add_run(usage, outcome, duration);
        self.per_model
            .entry(model.to_string())
            .or_default()
            .add_run(usage, outcome, duration);
        self.per_host
            .entry(host.to_string())
            .or_default()
            .add_run(model, usage, outcome, duration);
    }
}

/// The statistics file plus its in-memory image. Loaded once at startup;
/// `record_run` updates the counters and rewrites the file.
pub struct StatsStore {
    path: PathBuf,
    stats: Statistics,
}

impl StatsStore {
    pub fn load(data_dir: &str) -> StatsStore {
        let path = expand_tilde(data_dir).join(STATS_FILE);
        let stats = match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                eprintln!("cannot parse {}: {} — starting at zero", path.display(), e);
                Statistics::default()
            }),
            // no file yet — a fresh install is not an error
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Statistics::default(),
            Err(e) => {
                eprintln!("cannot read {}: {} — starting at zero", path.display(), e);
                Statistics::default()
            }
        };
        StatsStore { path, stats }
    }

    /// Update the counters with one finished run — overall, per model
    /// and per host — and persist them. A write failure is only reported
    /// on stderr — saving statistics must not break the run that just
    /// finished.
    pub fn record_run(
        &mut self,
        model: &str,
        host: &str,
        usage: &TokenUsage,
        outcome: &RunOutcome,
        duration: Duration,
    ) {
        self.stats.add_run(model, host, usage, outcome, duration);
        if let Err(e) = self.save() {
            eprintln!("could not save statistics: {}", e);
        }
    }

    /// Human-readable overall statistics for the `/stats` command,
    /// followed by one line per model and one line per host — each
    /// host line with one indented line per model used on it.
    pub fn summary(&self) -> String {
        let s = &self.stats.overall;
        if s.runs == 0 {
            return "no runs recorded yet.".to_string();
        }
        let since = s
            .first_run
            .as_deref()
            .and_then(rfc3339_date)
            .unwrap_or_else(|| "unknown date".to_string());
        let last = s
            .last_run
            .as_deref()
            .and_then(rfc3339_datetime)
            .unwrap_or_else(|| "unknown".to_string());
        let total = Duration::from_secs_f64(s.total_duration_secs);
        let per_run = Duration::from_secs_f64(s.total_duration_secs / s.runs as f64);
        let mut lines = vec![
            format!("overall statistics — {} runs since {}", s.runs, since),
            format!(
                "  finished: {}  errors: {}  round caps: {}",
                s.runs.saturating_sub(s.errors.saturating_add(s.round_caps)),
                s.errors,
                s.round_caps
            ),
            format!("  rounds: {}  tool calls: {}", s.rounds, s.tool_calls),
            format!(
                "  tokens: {} in, {} out ({} total)",
                format_tokens(s.prompt_tokens),
                format_tokens(s.completion_tokens),
                format_tokens(s.prompt_tokens + s.completion_tokens)
            ),
            format!(
                "  time: {} total, {} per run",
                format_duration(total),
                format_duration(per_run)
            ),
            format!("  last run: {}", last),
        ];
        lines.extend(bucket_lines("per model", &self.stats.per_model));
        lines.extend(host_lines(&self.stats.per_host));
        lines.join("\n")
    }

    fn save(&self) -> Result<(), String> {
        // the data dir may not exist yet — statistics are written after
        // the very first run, possibly before any skill or the history
        // file has created it
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        let content = serde_json::to_string_pretty(&self.stats)
            .map_err(|e| format!("cannot serialize statistics: {}", e))?;
        fs::write(&self.path, content)
            .map_err(|e| format!("cannot write {}: {}", self.path.display(), e))
    }
}

/// The `per host:` section of the summary — one line per host with its
/// totals, each followed by one indented line per model used on that
/// host. Hosts with no runs cannot exist, so no filtering is needed.
fn host_lines(hosts: &BTreeMap<String, HostCounters>) -> Vec<String> {
    if hosts.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["per host:".to_string()];
    for (host, counters) in hosts {
        lines.push(format!("  {}: {}", host, counters.total.summary_line()));
        lines.extend(
            counters
                .per_model
                .iter()
                .map(|(model, c)| format!("    {}: {}", model, c.summary_line())),
        );
    }
    lines
}

/// The `per model:` section of the summary — one line per
/// bucket, sorted by key (BTreeMap order). Buckets with no runs cannot
/// exist, so no filtering is needed.
fn bucket_lines(title: &str, buckets: &BTreeMap<String, RunCounters>) -> Vec<String> {
    if buckets.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![format!("{}:", title)];
    lines.extend(
        buckets
            .iter()
            .map(|(key, counters)| format!("  {}: {}", key, counters.summary_line())),
    );
    lines
}

/// `2026-10-09` from an RFC 3339 timestamp; `None` when it does not
/// parse, so a hand-edited file cannot break the command.
fn rfc3339_date(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// `2026-10-09 11:12` from an RFC 3339 timestamp.
fn rfc3339_datetime(raw: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp dir per test label; removed first so re-runs are clean.
    fn temp_data_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sven-stats-{}-{}",
            std::process::id(),
            label
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn outcome(status: RunStatus) -> RunOutcome {
        RunOutcome {
            rounds: 2,
            tool_calls: 3,
            status,
        }
    }

    #[test]
    fn missing_file_starts_at_zero() {
        let dir = temp_data_dir("missing");
        let store = StatsStore::load(dir.to_str().unwrap());
        assert_eq!(store.stats, Statistics::default());
        assert_eq!(store.summary(), "no runs recorded yet.");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_run_accumulates_and_persists() {
        let dir = temp_data_dir("record");
        let mut store = StatsStore::load(dir.to_str().unwrap());
        store.record_run(
            "gemma4:12b",
            "http://localhost:11434",
            &TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 20,
            },
            &outcome(RunStatus::Finished),
            Duration::from_secs(10),
        );
        store.record_run(
            "gemma4:12b",
            "http://localhost:11434",
            &TokenUsage {
                prompt_tokens: 50,
                completion_tokens: 5,
            },
            &outcome(RunStatus::Error),
            Duration::from_secs(5),
        );

        // the file is rewritten after every run, so a fresh store sees
        // the same counters
        let reloaded = StatsStore::load(dir.to_str().unwrap());
        assert_eq!(reloaded.stats.overall.runs, 2);
        assert_eq!(reloaded.stats.overall.errors, 1);
        assert_eq!(reloaded.stats.overall.round_caps, 0);
        assert_eq!(reloaded.stats.overall.rounds, 4);
        assert_eq!(reloaded.stats.overall.tool_calls, 6);
        assert_eq!(reloaded.stats.overall.prompt_tokens, 150);
        assert_eq!(reloaded.stats.overall.completion_tokens, 25);
        assert!((reloaded.stats.overall.total_duration_secs - 15.0).abs() < 1e-9);
        assert!(reloaded.stats.overall.first_run.is_some());
        assert!(reloaded.stats.overall.last_run.is_some());

        let summary = reloaded.summary();
        assert!(summary.contains("2 runs since"));
        assert!(summary.contains("errors: 1"));
        assert!(summary.contains("rounds: 4"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runs_are_bucketed_per_model_and_host() {
        let dir = temp_data_dir("buckets");
        let mut store = StatsStore::load(dir.to_str().unwrap());
        store.record_run(
            "gemma4:12b",
            "http://localhost:11434",
            &TokenUsage {
                prompt_tokens: 100,
                completion_tokens: 20,
            },
            &outcome(RunStatus::Finished),
            Duration::from_secs(10),
        );
        store.record_run(
            "qwen:7b",
            "http://localhost:11434",
            &TokenUsage {
                prompt_tokens: 50,
                completion_tokens: 5,
            },
            &outcome(RunStatus::Finished),
            Duration::from_secs(5),
        );
        store.record_run(
            "qwen:7b",
            "http://gpu-box:8000",
            &TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 2,
            },
            &outcome(RunStatus::RoundCap),
            Duration::from_secs(1),
        );

        let reloaded = StatsStore::load(dir.to_str().unwrap());
        // overall sees all three runs, each bucket only its own
        assert_eq!(reloaded.stats.overall.runs, 3);
        let gemma = &reloaded.stats.per_model["gemma4:12b"];
        assert_eq!(gemma.runs, 1);
        assert_eq!(gemma.prompt_tokens, 100);
        let qwen = &reloaded.stats.per_model["qwen:7b"];
        assert_eq!(qwen.runs, 2);
        assert_eq!(qwen.prompt_tokens, 60);
        assert_eq!(qwen.round_caps, 1);
        let local = &reloaded.stats.per_host["http://localhost:11434"];
        assert_eq!(local.total.runs, 2);
        assert_eq!(local.total.prompt_tokens, 150);
        let gpu = &reloaded.stats.per_host["http://gpu-box:8000"];
        assert_eq!(gpu.total.runs, 1);
        assert_eq!(gpu.total.completion_tokens, 2);

        // each host breaks its runs down per model
        assert_eq!(local.per_model["gemma4:12b"].runs, 1);
        assert_eq!(local.per_model["qwen:7b"].runs, 1);
        assert_eq!(local.per_model["qwen:7b"].prompt_tokens, 50);
        assert_eq!(gpu.per_model["qwen:7b"].runs, 1);
        assert_eq!(gpu.per_model["qwen:7b"].round_caps, 1);

        // the summary lists every bucket, hosts with their models
        let summary = reloaded.summary();
        assert!(summary.contains("per model:"));
        assert!(summary.contains("gemma4:12b: 1 runs, 100 in, 20 out, 10.0s"));
        assert!(summary.contains("qwen:7b: 2 runs, 60 in, 7 out, 6.0s"));
        assert!(summary.contains("per host:"));
        assert!(summary.contains("http://localhost:11434: 2 runs, 150 in, 25 out, 15.0s"));
        assert!(summary.contains("    gemma4:12b: 1 runs, 100 in, 20 out, 10.0s"));
        assert!(summary.contains("    qwen:7b: 1 runs, 50 in, 5 out, 5.0s"));
        assert!(summary.contains("http://gpu-box:8000: 1 runs, 10 in, 2 out, 1.0s"));
        assert!(summary.contains("    qwen:7b: 1 runs, 10 in, 2 out, 1.0s"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_reported_and_starts_at_zero() {
        let dir = temp_data_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(STATS_FILE), "not json").unwrap();
        let store = StatsStore::load(dir.to_str().unwrap());
        assert_eq!(store.stats, Statistics::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn older_schema_files_still_load() {
        // a file written before a field existed must still load
        let dir = temp_data_dir("older");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(STATS_FILE), r#"{"runs": 7, "prompt_tokens": 123}"#).unwrap();
        let store = StatsStore::load(dir.to_str().unwrap());
        assert_eq!(store.stats.overall.runs, 7);
        assert_eq!(store.stats.overall.prompt_tokens, 123);
        assert_eq!(store.stats.overall.tool_calls, 0);
        assert_eq!(store.stats.overall.first_run, None);
        assert!(store.stats.per_model.is_empty());
        assert!(store.stats.per_host.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pre_flatten_schema_files_still_load() {
        // files written before the per-model/per-host change have the
        // counters at the top level; `#[serde(flatten)]` picks them up
        // as `overall`
        let dir = temp_data_dir("pre-flatten");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(STATS_FILE),
            r#"{"runs": 3, "errors": 1, "rounds": 9, "tool_calls": 12,
                "prompt_tokens": 400, "completion_tokens": 60,
                "total_duration_secs": 42.5}"#,
        )
        .unwrap();
        let store = StatsStore::load(dir.to_str().unwrap());
        assert_eq!(store.stats.overall.runs, 3);
        assert_eq!(store.stats.overall.errors, 1);
        assert_eq!(store.stats.overall.rounds, 9);
        assert_eq!(store.stats.overall.tool_calls, 12);
        assert!((store.stats.overall.total_duration_secs - 42.5).abs() < 1e-9);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pre_host_per_model_schema_files_still_load() {
        // files written before the per-model-per-host change have plain
        // `RunCounters` per host; the flattened totals are kept and the
        // per-model sections start empty
        let dir = temp_data_dir("pre-host-model");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(STATS_FILE),
            r#"{"runs": 2, "per_host": {"http://localhost:11434":
                {"runs": 2, "prompt_tokens": 150, "completion_tokens": 25}}}"#,
        )
        .unwrap();
        let store = StatsStore::load(dir.to_str().unwrap());
        assert_eq!(store.stats.overall.runs, 2);
        let local = &store.stats.per_host["http://localhost:11434"];
        assert_eq!(local.total.runs, 2);
        assert_eq!(local.total.prompt_tokens, 150);
        assert!(local.per_model.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}