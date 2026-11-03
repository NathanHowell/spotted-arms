//! Pure (sans-IO) runner lifecycle state machine.
//!
//! One [`Handler`] is created per webhook delivery. It never performs I/O: it
//! returns [`Effect`]s describing what the outside world should do, and is
//! fed the results as [`Response`]s. The async interpreter lives in
//! `instance.rs`; a simulated world lives in `tests/world.rs`.
//!
//! The states mirror the handler program counters in `tla/SpottedArms.tla`
//! (`jit` + template fetch -> `insert` -> done, and `delete` -> done).
//!
//! Key invariant (see the TLA+ model): runners are fungible. GitHub gives any
//! idle runner with matching labels to any queued job, so the VM named for a
//! job is *not* necessarily the VM that ran it. Deletion therefore targets the
//! `runner_name` GitHub reports for the completed job, never a name derived
//! from the job id.

use crate::utils::instance_name;
use http::StatusCode;
use reqwest::Url;

pub const REQUIRED_LABELS: &[&str] = &["linux", "self-hosted", "ARM64"];

// Supported zones for the us-central1 region
const US_CENTRAL1_ZONES: &[&str] = &[
    "us-central1-a",
    "us-central1-b",
    "us-central1-c",
    "us-central1-f",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobAction {
    Queued,
    Completed,
    Other,
}

/// The subset of a `workflow_job` webhook the state machine cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobEvent {
    pub action: JobAction,
    pub repo_url: Url,
    pub run_id: i64,
    pub job_id: i64,
    pub labels: Vec<String>,
    /// Name of the runner that executed the job. `None` for jobs cancelled
    /// before any runner picked them up.
    pub runner_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub region: String,
}

/// Something the outside world must do on the machine's behalf.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Effect {
    GenerateJit {
        runner_name: String,
        labels: Vec<String>,
    },
    GetTemplateMetadata,
    InsertInstance {
        name: String,
        zone: String,
        /// Template metadata with `JIT_CONFIG` appended.
        metadata: Vec<(String, String)>,
    },
    DeleteInstance {
        name: String,
        zone: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EffectError {
    NotFound,
    Other(String),
}

/// The result of running one [`Effect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Jit(Result<String, EffectError>),
    TemplateMetadata(Result<Vec<(String, String)>, EffectError>),
    Inserted(Result<(), EffectError>),
    Deleted(Result<(), EffectError>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Deleted,
    /// Delete found nothing (already gone, e.g. the VM powered itself off).
    AlreadyGone,
    Ignored(&'static str),
    Failed {
        status: StatusCode,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Run these effects (concurrently) and feed every result back via
    /// [`Handler::resume`]. An empty list means "waiting for more responses".
    Do(Vec<Effect>),
    Done(Outcome),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum State {
    Preparing {
        jit: Option<String>,
        template: Option<Vec<(String, String)>>,
    },
    Inserting,
    Deleting,
    Finished,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Handler {
    name: String,
    zone: String,
    state: State,
}

/// Jobs must carry every required label (GitHub matches labels
/// case-insensitively).
pub fn has_required_labels(labels: &[String]) -> bool {
    REQUIRED_LABELS
        .iter()
        .all(|req| labels.iter().any(|l| l.eq_ignore_ascii_case(req)))
}

/// Deterministically selects a zone from the instance name.
///
/// Uses FNV-1a over the name's bytes rather than `std::hash::DefaultHasher`,
/// whose algorithm is explicitly not guaranteed across Rust releases. The zone
/// is *recomputed* from the instance name when deleting, so create and delete
/// must always agree, whatever toolchain built each.
pub fn select_zone_for_region(region: &str, instance_name: &str) -> Result<String, String> {
    use std::hash::Hasher;

    if region != "us-central1" {
        return Err(format!(
            "Unsupported region: {region}. Only us-central1 is currently supported."
        ));
    }

    let mut hasher = fnv::FnvHasher::default();
    hasher.write(instance_name.as_bytes());
    let index = (hasher.finish() % US_CENTRAL1_ZONES.len() as u64) as usize;
    Ok(US_CENTRAL1_ZONES[index].to_string())
}

/// True for names produced by [`instance_name`] (`gha-<run_id>-<job_id>`).
/// The completed webhook is attacker-influenced data (any self-hosted runner
/// with our labels can appear in it), so never delete anything else.
pub fn is_managed_runner(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("gha-") else {
        return false;
    };
    let mut parts = rest.split('-');
    let digits =
        |p: Option<&str>| p.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

fn done(outcome: Outcome) -> Step {
    Step::Done(outcome)
}

fn failed(status: StatusCode, message: impl Into<String>) -> Step {
    Step::Done(Outcome::Failed {
        status,
        message: message.into(),
    })
}

impl Handler {
    /// Begin handling a webhook delivery.
    pub fn start(cfg: &Config, event: &JobEvent) -> (Handler, Step) {
        let mut handler = Handler {
            name: String::new(),
            zone: String::new(),
            state: State::Finished,
        };
        let step = handler.begin(cfg, event);
        (handler, step)
    }

    fn begin(&mut self, cfg: &Config, event: &JobEvent) -> Step {
        if !has_required_labels(&event.labels) {
            return done(Outcome::Ignored("missing required labels"));
        }

        match event.action {
            JobAction::Queued => {
                if event.repo_url.host_str() != Some("api.github.com") {
                    return failed(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Invalid repository URL format",
                    );
                }
                let name = instance_name(event.run_id, event.job_id);
                // Validate the region before registering a runner with GitHub.
                let zone = match select_zone_for_region(&cfg.region, &name) {
                    Ok(zone) => zone,
                    Err(message) => return failed(StatusCode::BAD_REQUEST, message),
                };
                self.name = name.clone();
                self.zone = zone;
                self.state = State::Preparing {
                    jit: None,
                    template: None,
                };
                Step::Do(vec![
                    Effect::GenerateJit {
                        runner_name: name,
                        labels: event.labels.clone(),
                    },
                    Effect::GetTemplateMetadata,
                ])
            }
            JobAction::Completed => {
                // Delete the runner that actually ran the job. It is not
                // necessarily the VM named after this job.
                let Some(runner) = event.runner_name.as_deref() else {
                    return done(Outcome::Ignored("job never had a runner"));
                };
                if !is_managed_runner(runner) {
                    return done(Outcome::Ignored("runner is not managed by this service"));
                }
                let zone = match select_zone_for_region(&cfg.region, runner) {
                    Ok(zone) => zone,
                    Err(message) => return failed(StatusCode::BAD_REQUEST, message),
                };
                self.name = runner.to_string();
                self.zone = zone.clone();
                self.state = State::Deleting;
                Step::Do(vec![Effect::DeleteInstance {
                    name: runner.to_string(),
                    zone,
                }])
            }
            JobAction::Other => done(Outcome::Ignored("unhandled action")),
        }
    }

    /// Feed the result of a previously requested effect.
    pub fn resume(&mut self, response: Response) -> Step {
        let state = std::mem::replace(&mut self.state, State::Finished);
        match (state, response) {
            (State::Preparing { template, .. }, Response::Jit(result)) => match result {
                Ok(jit) => self.prepared(Some(jit), template),
                Err(_) => failed(StatusCode::INTERNAL_SERVER_ERROR, "jit config failed"),
            },
            (State::Preparing { jit, .. }, Response::TemplateMetadata(result)) => match result {
                Ok(template) => self.prepared(jit, Some(template)),
                Err(_) => failed(StatusCode::INTERNAL_SERVER_ERROR, "templates get failed"),
            },
            (State::Inserting, Response::Inserted(result)) => match result {
                Ok(()) => done(Outcome::Created),
                Err(e) => failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:?}")),
            },
            (State::Deleting, Response::Deleted(result)) => match result {
                Ok(()) => done(Outcome::Deleted),
                Err(EffectError::NotFound) => done(Outcome::AlreadyGone),
                Err(e) => failed(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:?}")),
            },
            (_, response) => failed(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("unexpected response: {response:?}"),
            ),
        }
    }

    fn prepared(&mut self, jit: Option<String>, template: Option<Vec<(String, String)>>) -> Step {
        match (jit, template) {
            (Some(jit), Some(mut metadata)) => {
                metadata.push(("JIT_CONFIG".to_string(), jit));
                self.state = State::Inserting;
                Step::Do(vec![Effect::InsertInstance {
                    name: self.name.clone(),
                    zone: self.zone.clone(),
                    metadata,
                }])
            }
            (jit, template) => {
                self.state = State::Preparing { jit, template };
                Step::Do(Vec::new())
            }
        }
    }

    pub fn is_finished(&self) -> bool {
        self.state == State::Finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED_ZONES: &[(&str, &str)] = &[
        ("gha-1-2", "us-central1-a"),
        ("gha-123-42", "us-central1-b"),
        ("gha-123-7", "us-central1-a"),
        ("gha-10-20", "us-central1-c"),
        ("gha-9876543210-1234567890", "us-central1-b"),
    ];

    fn cfg() -> Config {
        Config {
            region: "us-central1".into(),
        }
    }

    fn event(action: JobAction, runner_name: Option<&str>) -> JobEvent {
        JobEvent {
            action,
            repo_url: Url::parse("https://api.github.com/repos/o/r").unwrap(),
            run_id: 10,
            job_id: 20,
            labels: vec!["self-hosted".into(), "linux".into(), "ARM64".into()],
            runner_name: runner_name.map(str::to_string),
        }
    }

    fn effects(step: Step) -> Vec<Effect> {
        match step {
            Step::Do(effects) => effects,
            other => panic!("expected effects, got {other:?}"),
        }
    }

    #[test]
    fn queued_runs_jit_then_template_then_insert() {
        let (mut h, step) = Handler::start(&cfg(), &event(JobAction::Queued, None));
        let fx = effects(step);
        assert!(
            matches!(&fx[0], Effect::GenerateJit { runner_name, .. } if runner_name == "gha-10-20")
        );
        assert_eq!(fx[1], Effect::GetTemplateMetadata);

        // responses may arrive in either order; nothing happens until both do
        assert_eq!(h.resume(Response::Jit(Ok("jit".into()))), Step::Do(vec![]));
        let fx = effects(h.resume(Response::TemplateMetadata(Ok(vec![(
            "a".into(),
            "b".into(),
        )]))));
        let Effect::InsertInstance { name, metadata, .. } = &fx[0] else {
            panic!("expected insert, got {fx:?}");
        };
        assert_eq!(name, "gha-10-20");
        assert_eq!(
            metadata,
            &vec![
                ("a".into(), "b".into()),
                ("JIT_CONFIG".into(), "jit".into())
            ]
        );
        assert_eq!(
            h.resume(Response::Inserted(Ok(()))),
            Step::Done(Outcome::Created)
        );
    }

    #[test]
    fn completed_deletes_the_runner_that_ran_the_job_not_the_jobs_own_name() {
        // job 20 ran on the VM that was created for some other job
        let (_, step) = Handler::start(&cfg(), &event(JobAction::Completed, Some("gha-10-99")));
        let fx = effects(step);
        assert!(matches!(&fx[0], Effect::DeleteInstance { name, .. } if name == "gha-10-99"));
    }

    #[test]
    fn completed_without_runner_deletes_nothing() {
        let (_, step) = Handler::start(&cfg(), &event(JobAction::Completed, None));
        assert!(matches!(step, Step::Done(Outcome::Ignored(_))));
    }

    #[test]
    fn completed_with_foreign_runner_deletes_nothing() {
        for name in [
            "my-laptop",
            "gha-1",
            "gha-1-2-3",
            "gha--2",
            "gha-a-2",
            "x-gha-1-2",
        ] {
            let (_, step) = Handler::start(&cfg(), &event(JobAction::Completed, Some(name)));
            assert!(matches!(step, Step::Done(Outcome::Ignored(_))), "{name}");
        }
    }

    #[test]
    fn delete_not_found_is_already_gone() {
        let (mut h, _) = Handler::start(&cfg(), &event(JobAction::Completed, Some("gha-1-2")));
        assert_eq!(
            h.resume(Response::Deleted(Err(EffectError::NotFound))),
            Step::Done(Outcome::AlreadyGone)
        );
    }

    #[test]
    fn delete_error_fails() {
        let (mut h, _) = Handler::start(&cfg(), &event(JobAction::Completed, Some("gha-1-2")));
        let step = h.resume(Response::Deleted(Err(EffectError::Other("boom".into()))));
        assert!(matches!(
            step,
            Step::Done(Outcome::Failed {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                ..
            })
        ));
    }

    #[test]
    fn unsupported_region_fails_before_any_effect() {
        let cfg = Config {
            region: "europe-west1".into(),
        };
        for action in [JobAction::Queued, JobAction::Completed] {
            let (_, step) = Handler::start(&cfg, &event(action, Some("gha-1-2")));
            assert!(matches!(
                step,
                Step::Done(Outcome::Failed {
                    status: StatusCode::BAD_REQUEST,
                    ..
                })
            ));
        }
    }

    #[test]
    fn missing_labels_are_ignored_and_matching_is_case_insensitive() {
        let mut e = event(JobAction::Queued, None);
        e.labels = vec!["self-hosted".into(), "linux".into()];
        assert!(matches!(
            Handler::start(&cfg(), &e).1,
            Step::Done(Outcome::Ignored(_))
        ));

        e.labels = vec!["Self-Hosted".into(), "LINUX".into(), "arm64".into()];
        assert!(matches!(Handler::start(&cfg(), &e).1, Step::Do(_)));
    }

    #[test]
    fn non_github_repo_url_is_rejected() {
        let mut e = event(JobAction::Queued, None);
        e.repo_url = Url::parse("https://evil.example/repos/o/r").unwrap();
        assert!(matches!(
            Handler::start(&cfg(), &e).1,
            Step::Done(Outcome::Failed { .. })
        ));
    }

    #[test]
    fn jit_failure_fails_and_never_inserts() {
        let (mut h, _) = Handler::start(&cfg(), &event(JobAction::Queued, None));
        let step = h.resume(Response::Jit(Err(EffectError::Other("nope".into()))));
        assert!(matches!(step, Step::Done(Outcome::Failed { .. })));
        assert!(h.is_finished());
    }

    #[test]
    fn other_actions_are_ignored() {
        let (_, step) = Handler::start(&cfg(), &event(JobAction::Other, None));
        assert!(matches!(step, Step::Done(Outcome::Ignored(_))));
    }

    /// Pinned values: if these change, instances created by an older build
    /// would be deleted in the wrong zone (and the NotFound swallowed).
    #[test]
    fn zone_selection_is_pinned() {
        let zone = |name| select_zone_for_region("us-central1", name).unwrap();
        for (name, expected) in PINNED_ZONES {
            assert_eq!(zone(name), *expected, "{name}");
        }
    }
}
