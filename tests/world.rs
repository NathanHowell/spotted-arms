//! A simulated GitHub + GCE world that drives the real `machine::Handler`.
//!
//! This is the Rust counterpart of `tla/SpottedArms.tla`: GitHub gives any
//! idle runner to any queued job, webhooks arrive in any order, handlers
//! interleave one effect at a time. Scripted tests replay the TLC
//! counterexamples; the exhaustive tests explore every interleaving and check
//! the same invariants (`NoKilledJob`, `NoStuckJob`).

use http::StatusCode;
use reqwest::Url;
use spotted_arms::machine::{
    Config, Effect, EffectError, Handler, JobAction, JobEvent, Outcome, Response, Step,
};
use spotted_arms::utils::instance_name;
use std::collections::{BTreeMap, BTreeSet, HashSet};

type JobId = i64;
const RUN: i64 = 1;

fn runner(job: JobId) -> String {
    instance_name(RUN, job)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Job {
    Idle,
    Queued,
    Running(String),
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Vm {
    Idle,
    Busy(JobId),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Ev {
    Queued(JobId),
    Completed(JobId, Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Pending {
    handler: Handler,
    inflight: Vec<Effect>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct World {
    jobs: BTreeMap<JobId, Job>,
    vms: BTreeMap<String, Vm>,
    registered: BTreeSet<String>,
    emitted: BTreeSet<Ev>,
    delivered: BTreeSet<Ev>,
    handlers: BTreeMap<Ev, Pending>,
    killed: BTreeSet<JobId>,
    failures: Vec<StatusCode>,
}

fn cfg() -> Config {
    Config {
        region: "us-central1".into(),
    }
}

fn job_event(ev: &Ev) -> JobEvent {
    let (action, job_id, runner_name) = match ev {
        Ev::Queued(j) => (JobAction::Queued, *j, None),
        Ev::Completed(j, r) => (JobAction::Completed, *j, r.clone()),
    };
    JobEvent {
        action,
        repo_url: Url::parse("https://api.github.com/repos/o/r").unwrap(),
        run_id: RUN,
        job_id,
        labels: vec!["self-hosted".into(), "linux".into(), "ARM64".into()],
        runner_name,
    }
}

impl World {
    fn new(jobs: &[JobId]) -> World {
        World {
            jobs: jobs.iter().map(|&j| (j, Job::Idle)).collect(),
            ..World::default()
        }
    }

    // ---- GitHub / VM side -------------------------------------------------

    fn enqueue(&mut self, j: JobId) {
        assert_eq!(self.jobs[&j], Job::Idle);
        self.jobs.insert(j, Job::Queued);
        self.emitted.insert(Ev::Queued(j));
    }

    fn cancel(&mut self, j: JobId) {
        assert_eq!(self.jobs[&j], Job::Queued);
        self.jobs.insert(j, Job::Done);
        self.emitted.insert(Ev::Completed(j, None));
    }

    /// Any idle runner picks any queued job (labels are identical).
    fn pick(&mut self, r: &str, j: JobId) {
        assert_eq!(self.vms[r], Vm::Idle);
        assert_eq!(self.jobs[&j], Job::Queued);
        self.jobs.insert(j, Job::Running(r.to_string()));
        self.vms.insert(r.to_string(), Vm::Busy(j));
    }

    /// Job ends; the ephemeral runner exits and the VM powers off and deletes itself.
    fn finish(&mut self, j: JobId) {
        let Job::Running(r) = self.jobs[&j].clone() else {
            panic!("job {j} is not running")
        };
        self.jobs.insert(j, Job::Done);
        self.vms.remove(&r);
        self.registered.remove(&r);
        self.emitted.insert(Ev::Completed(j, Some(r)));
    }

    // ---- webhook delivery and handlers -----------------------------------

    fn deliver(&mut self, ev: &Ev) {
        assert!(self.emitted.contains(ev) && !self.delivered.contains(ev));
        self.delivered.insert(ev.clone());
        let (handler, step) = Handler::start(&cfg(), &job_event(ev));
        self.apply(ev, handler, step, None);
    }

    fn apply(&mut self, ev: &Ev, handler: Handler, step: Step, executed: Option<usize>) {
        let mut inflight = self
            .handlers
            .remove(ev)
            .map(|p| p.inflight)
            .unwrap_or_default();
        if let Some(i) = executed {
            inflight.remove(i);
        }
        match step {
            Step::Done(outcome) => {
                if let Outcome::Failed { status, .. } = outcome {
                    self.failures.push(status);
                }
            }
            Step::Do(effects) => {
                inflight.extend(effects);
                self.handlers
                    .insert(ev.clone(), Pending { handler, inflight });
            }
        }
    }

    /// Execute one in-flight effect of one handler. `fault` makes the external
    /// call fail with a server error.
    fn step_handler(&mut self, ev: &Ev, idx: usize, fault: bool) {
        let Pending {
            mut handler,
            inflight,
        } = self.handlers[ev].clone();
        let response = self.exec(&inflight[idx], fault);
        let step = handler.resume(response);
        self.apply(ev, handler, step, Some(idx));
    }

    fn exec(&mut self, effect: &Effect, fault: bool) -> Response {
        let err = || EffectError::Other("injected fault".into());
        match effect {
            Effect::GenerateJit { runner_name, .. } => Response::Jit(if fault {
                Err(err())
            } else if !self.registered.insert(runner_name.clone()) {
                Err(EffectError::Other("409 runner exists".into()))
            } else {
                Ok("jit".into())
            }),
            Effect::GetTemplateMetadata => {
                Response::TemplateMetadata(if fault { Err(err()) } else { Ok(vec![]) })
            }
            Effect::InsertInstance { name, .. } => Response::Inserted(if fault {
                Err(err())
            } else if self.vms.contains_key(name) {
                Err(EffectError::Other("409 instance exists".into()))
            } else {
                self.vms.insert(name.clone(), Vm::Idle);
                Ok(())
            }),
            Effect::DeleteInstance { name, .. } => Response::Deleted(match self.vms.remove(name) {
                None => Err(EffectError::NotFound),
                Some(Vm::Idle) => Ok(()),
                Some(Vm::Busy(j)) => {
                    // the job running on the deleted VM dies
                    self.killed.insert(j);
                    self.jobs.insert(j, Job::Done);
                    self.registered.remove(name);
                    self.emitted.insert(Ev::Completed(j, Some(name.clone())));
                    Ok(())
                }
            }),
        }
    }

    /// Run every handler to completion, one effect at a time.
    fn settle(&mut self) {
        while let Some(ev) = self.handlers.keys().next().cloned() {
            self.step_handler(&ev, 0, false);
        }
    }

    fn deliver_all(&mut self) {
        while let Some(ev) = self.emitted.difference(&self.delivered).next().cloned() {
            self.deliver(&ev);
            self.settle();
        }
    }

    // ---- invariants -------------------------------------------------------

    fn quiescent(&self) -> bool {
        self.emitted == self.delivered && self.handlers.is_empty()
    }

    fn violation(&self, check_stuck: bool) -> Option<&'static str> {
        if !self.killed.is_empty() {
            return Some("NoKilledJob: a VM was deleted while running a job");
        }
        if check_stuck
            && self.quiescent()
            && self.jobs.values().any(|j| *j == Job::Queued)
            && !self.vms.values().any(|v| *v == Vm::Idle)
        {
            return Some("NoStuckJob: a job is queued and no runner can take it");
        }
        None
    }
}

// ---- exhaustive exploration ---------------------------------------------

struct Opts {
    jobs: Vec<JobId>,
    cancel: bool,
    faults: bool,
    check_stuck: bool,
}

fn successors(w: &World, opts: &Opts) -> Vec<(String, World)> {
    let mut out = Vec::new();
    let mut add = |label: String, f: &dyn Fn(&mut World)| {
        let mut next = w.clone();
        f(&mut next);
        out.push((label, next));
    };

    for (&j, state) in &w.jobs {
        match state {
            Job::Idle => add(format!("enqueue {j}"), &|n| n.enqueue(j)),
            Job::Queued => {
                if opts.cancel {
                    add(format!("cancel {j}"), &|n| n.cancel(j));
                }
                for (r, vm) in &w.vms {
                    if *vm == Vm::Idle {
                        add(format!("{r} picks job {j}"), &|n| n.pick(r, j));
                    }
                }
            }
            Job::Running(_) => add(format!("finish {j}"), &|n| n.finish(j)),
            Job::Done => {}
        }
    }
    for ev in w.emitted.difference(&w.delivered) {
        add(format!("deliver {ev:?}"), &|n| n.deliver(ev));
    }
    for (ev, pending) in &w.handlers {
        for idx in 0..pending.inflight.len() {
            add(format!("{ev:?}: run {:?}", pending.inflight[idx]), &|n| {
                n.step_handler(ev, idx, false)
            });
            if opts.faults {
                add(format!("{ev:?}: FAULT {:?}", pending.inflight[idx]), &|n| {
                    n.step_handler(ev, idx, true)
                });
            }
        }
    }
    out
}

/// Explore every reachable state; return the first violation with its trace.
fn explore(opts: &Opts) -> Result<usize, String> {
    let init = World::new(&opts.jobs);
    let mut seen: HashSet<World> = HashSet::from([init.clone()]);
    let mut stack = vec![(init, Vec::<String>::new())];
    while let Some((w, trace)) = stack.pop() {
        if let Some(v) = w.violation(opts.check_stuck) {
            return Err(format!("{v}\n  {}", trace.join("\n  ")));
        }
        for (label, next) in successors(&w, opts) {
            if seen.insert(next.clone()) {
                let mut t = trace.clone();
                t.push(label);
                stack.push((next, t));
            }
        }
    }
    Ok(seen.len())
}

// ---- scripted regressions (TLC counterexamples) --------------------------

/// `kill_basic`: runners swap jobs. Job 1 runs on the VM created for job 2 and
/// vice versa. When job 1 completes, only the VM that ran it may be touched.
#[test]
fn swapped_runners_do_not_kill_the_other_job() {
    let mut w = World::new(&[1, 2]);
    w.enqueue(1);
    w.enqueue(2);
    w.deliver_all(); // both VMs created
    w.pick(&runner(2), 1);
    w.pick(&runner(1), 2);

    w.finish(1); // job 1 ran on runner(2); runner(1) still runs job 2
    w.deliver_all(); // completed(1, runner(2))

    assert!(w.killed.is_empty(), "killed: {:?}", w.killed);
    assert_eq!(w.vms[&runner(1)], Vm::Busy(2));
    assert!(w.failures.is_empty());
}

/// `kill_cancel`: a job cancelled while queued has no runner. Its own VM may
/// meanwhile be running another job and must not be deleted.
#[test]
fn cancelled_job_does_not_kill_the_runner_serving_another_job() {
    let mut w = World::new(&[1, 2]);
    w.enqueue(1);
    w.enqueue(2);
    w.cancel(1);
    w.deliver_all(); // runner(1) and runner(2) are created
    w.pick(&runner(1), 2); // runner(1) was created for the cancelled job 1

    w.deliver_all(); // completed(1, None)
    assert!(w.killed.is_empty(), "killed: {:?}", w.killed);
    assert_eq!(w.vms[&runner(1)], Vm::Busy(2));
}

/// A completed event that arrives after the VM already powered itself off
/// must be harmless.
#[test]
fn completed_after_self_delete_is_a_noop() {
    let mut w = World::new(&[1]);
    w.enqueue(1);
    w.deliver_all();
    w.pick(&runner(1), 1);
    w.finish(1);
    w.deliver_all();
    assert!(w.killed.is_empty());
    assert!(w.vms.is_empty());
    assert!(w.failures.is_empty());
}

// ---- exhaustive: the Rust handlers against the TLA+ invariants -----------

#[test]
fn exhaustive_two_jobs_with_cancellation_and_faults_never_kill_a_job() {
    let n = explore(&Opts {
        jobs: vec![1, 2],
        cancel: true,
        faults: true,
        check_stuck: false,
    })
    .unwrap_or_else(|trace| panic!("{trace}"));
    println!("explored {n} states");
}

#[test]
fn exhaustive_two_jobs_with_cancellation_never_strand_a_job() {
    let n = explore(&Opts {
        jobs: vec![1, 2],
        cancel: true,
        faults: false,
        check_stuck: true,
    })
    .unwrap_or_else(|trace| panic!("{trace}"));
    println!("explored {n} states");
}

#[test]
fn exhaustive_three_jobs_never_kill_or_strand_a_job() {
    let n = explore(&Opts {
        jobs: vec![1, 2, 3],
        cancel: false,
        faults: false,
        check_stuck: true,
    })
    .unwrap_or_else(|trace| panic!("{trace}"));
    println!("explored {n} states");
}

/// Known limitation, same as TLC's `stuck_faults`: a failed JIT/insert is not
/// retried (GitHub does not redeliver on 5xx), so the job is stranded.
#[test]
fn faults_can_strand_a_job_known_limitation() {
    let result = explore(&Opts {
        jobs: vec![1],
        cancel: false,
        faults: true,
        check_stuck: true,
    });
    assert!(result.is_err_and(|t| t.contains("NoStuckJob")));
}
