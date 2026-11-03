------------------------------ MODULE SpottedArms ------------------------------
(***************************************************************************)
(* Model of the spotted-arms webhook -> GCE runner lifecycle.              *)
(*                                                                         *)
(* Code being modelled:                                                    *)
(*   webhook.rs   handle_workflow_job_event: queued -> create_instance,    *)
(*                                           completed -> delete_instance  *)
(*   instance.rs  create_instance: (1) generate JIT config  (2) insert VM  *)
(*                delete_instance: delete VM named from the *job*,         *)
(*                                 NotFound is treated as success          *)
(*   utils.rs     make_instance_name: gha-{run_id}-{job_id}                *)
(*                                                                         *)
(* GitHub semantics modelled:                                              *)
(*   - Any idle runner whose labels match may pick up any queued job.      *)
(*     (All our runners have identical labels.)                            *)
(*   - JIT runners are ephemeral: one job, then the registration is gone.  *)
(*   - Webhooks are delivered in arbitrary order, handlers run             *)
(*     concurrently, and any delivery may be redelivered.                  *)
(*   - The `completed` payload carries runner_name (null if the job was    *)
(*     cancelled before any runner picked it up).                          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    Jobs,            \* set of job ids
    DeleteBy,        \* "JobName"   = current code (name derived from event's job)
                     \* "RunnerName"= delete the runner that actually ran the job
    AllowCancel,     \* queued jobs may be cancelled before pickup
    AllowRedeliver,  \* each webhook may be delivered a second time
    AllowFaults      \* JIT / insert calls may fail (no retry in the service)

NONE == "none"

\* instance/runner name is a 1:1 function of the job (make_instance_name)
Names == Jobs

VARIABLES
    jobSt,       \* [Jobs -> {"idle","queued","running","done"}]   GitHub side
    jobRunner,   \* [Jobs -> Names \cup {NONE}]  runner that executed the job
    reg,         \* [Names -> BOOLEAN]  runner registered at GitHub (JIT config issued)
    vm,          \* [Names -> {"absent","up"}]  GCE instance exists
    runnerSt,    \* [Names -> {"none","idle","busy","exited"}]
    killed,      \* set of jobs whose VM was deleted while they were running
    emitted,     \* set of webhook events GitHub has produced
    delivered,   \* set of <<event, n>> deliveries made
    hs           \* set of handler records [ev, n, pc]

vars == <<jobSt, jobRunner, reg, vm, runnerSt, killed, emitted, delivered, hs>>

QueuedEv(j)       == [type |-> "queued",    job |-> j, runner |-> NONE]
CompletedEv(j, r) == [type |-> "completed", job |-> j, runner |-> r]

TypeOK ==
    /\ jobSt     \in [Jobs  -> {"idle", "queued", "running", "done"}]
    /\ jobRunner \in [Jobs  -> Names \cup {NONE}]
    /\ reg       \in [Names -> BOOLEAN]
    /\ vm        \in [Names -> {"absent", "up"}]
    /\ runnerSt  \in [Names -> {"none", "idle", "busy", "exited"}]
    /\ killed    \subseteq Jobs

Init ==
    /\ jobSt     = [j \in Jobs  |-> "idle"]
    /\ jobRunner = [j \in Jobs  |-> NONE]
    /\ reg       = [n \in Names |-> FALSE]
    /\ vm        = [n \in Names |-> "absent"]
    /\ runnerSt  = [n \in Names |-> "none"]
    /\ killed    = {}
    /\ emitted   = {}
    /\ delivered = {}
    /\ hs        = {}

(************************ GitHub / VM-side actions *************************)

Enqueue(j) ==
    /\ jobSt[j] = "idle"
    /\ jobSt' = [jobSt EXCEPT ![j] = "queued"]
    /\ emitted' = emitted \cup {QueuedEv(j)}
    /\ UNCHANGED <<jobRunner, reg, vm, runnerSt, killed, delivered, hs>>

Cancel(j) ==
    /\ AllowCancel
    /\ jobSt[j] = "queued"
    /\ jobSt' = [jobSt EXCEPT ![j] = "done"]
    /\ emitted' = emitted \cup {CompletedEv(j, NONE)}
    /\ UNCHANGED <<jobRunner, reg, vm, runnerSt, killed, delivered, hs>>

\* An idle runner picks ANY queued job (labels are identical).
RunnerPick(r, j) ==
    /\ vm[r] = "up" /\ runnerSt[r] = "idle"
    /\ jobSt[j] = "queued"
    /\ jobSt'    = [jobSt EXCEPT ![j] = "running"]
    /\ jobRunner' = [jobRunner EXCEPT ![j] = r]
    /\ runnerSt' = [runnerSt EXCEPT ![r] = "busy"]
    /\ UNCHANGED <<reg, vm, killed, emitted, delivered, hs>>

\* Job finishes normally; ephemeral runner exits and its registration vanishes.
Finish(j) ==
    LET r == jobRunner[j] IN
    /\ jobSt[j] = "running"
    /\ jobSt'    = [jobSt EXCEPT ![j] = "done"]
    /\ runnerSt' = [runnerSt EXCEPT ![r] = "exited"]
    /\ reg'      = [reg EXCEPT ![r] = FALSE]
    /\ emitted'  = emitted \cup {CompletedEv(j, r)}
    /\ UNCHANGED <<jobRunner, vm, killed, delivered, hs>>

(**************************** Webhook delivery *****************************)

Deliver(e, n) ==
    /\ e \in emitted
    /\ <<e, n>> \notin delivered
    /\ n = 1 \/ (AllowRedeliver /\ n = 2 /\ <<e, 1>> \in delivered)
    /\ delivered' = delivered \cup {<<e, n>>}
    /\ hs' = hs \cup {[ev |-> e, n |-> n,
                       pc |-> IF e.type = "queued" THEN "jit" ELSE "delete"]}
    /\ UNCHANGED <<jobSt, jobRunner, reg, vm, runnerSt, killed, emitted>>

(******************************** Handlers *********************************)

SetPc(h, pc) == hs' = (hs \ {h}) \cup {[h EXCEPT !.pc = pc]}

\* queued, step 1: github.generate_jit_config(runner_name = instance name)
HJit(h) ==
    LET n == h.ev.job IN
    /\ h.ev.type = "queued" /\ h.pc = "jit"
    /\ \/ /\ ~reg[n]
          /\ reg' = [reg EXCEPT ![n] = TRUE]
          /\ SetPc(h, "insert")
       \/ /\ reg[n]                       \* 409: runner name already registered
          /\ SetPc(h, "done") /\ UNCHANGED reg
       \/ /\ AllowFaults /\ ~reg[n]       \* 5xx from GitHub
          /\ SetPc(h, "done") /\ UNCHANGED reg
    /\ UNCHANGED <<jobSt, jobRunner, vm, runnerSt, killed, emitted, delivered>>

\* queued, step 2: compute.instances.insert (fire and forget; no operation wait)
HInsert(h) ==
    LET n == h.ev.job IN
    /\ h.ev.type = "queued" /\ h.pc = "insert"
    /\ \/ /\ vm[n] = "absent"
          /\ vm' = [vm EXCEPT ![n] = "up"]
          /\ runnerSt' = [runnerSt EXCEPT ![n] = "idle"]
          /\ SetPc(h, "done")
       \/ /\ vm[n] = "up"                 \* 409: instance already exists
          /\ SetPc(h, "done") /\ UNCHANGED <<vm, runnerSt>>
       \/ /\ AllowFaults                  \* quota / stockout / 5xx
          /\ SetPc(h, "done") /\ UNCHANGED <<vm, runnerSt>>
    /\ UNCHANGED <<jobSt, jobRunner, reg, killed, emitted, delivered>>

\* completed: compute.instances.delete.  NotFound is swallowed as success.
HDelete(h) ==
    LET target == IF DeleteBy = "JobName" THEN h.ev.job ELSE h.ev.runner
        victim == CHOOSE j \in Jobs : jobSt[j] = "running" /\ jobRunner[j] = target
        busy   == target # NONE /\ vm[target] = "up" /\ runnerSt[target] = "busy"
    IN
    /\ h.ev.type = "completed" /\ h.pc = "delete"
    /\ SetPc(h, "done")
    /\ IF target = NONE \/ vm[target] = "absent"
       THEN UNCHANGED <<jobSt, jobRunner, reg, vm, runnerSt, killed, emitted>>
       ELSE /\ vm' = [vm EXCEPT ![target] = "absent"]
            /\ runnerSt' = [runnerSt EXCEPT ![target] = "none"]
            /\ IF busy
               THEN \* job running on the deleted VM dies; GitHub reports it failed
                    /\ killed'  = killed \cup {victim}
                    /\ jobSt'   = [jobSt EXCEPT ![victim] = "done"]
                    /\ reg'     = [reg EXCEPT ![target] = FALSE]
                    /\ emitted' = emitted \cup {CompletedEv(victim, target)}
                    /\ UNCHANGED jobRunner
               ELSE UNCHANGED <<jobSt, jobRunner, reg, killed, emitted>>
    /\ UNCHANGED delivered

Next ==
    \/ \E j \in Jobs : Enqueue(j) \/ Cancel(j) \/ Finish(j)
    \/ \E r \in Names, j \in Jobs : RunnerPick(r, j)
    \/ \E e \in emitted, n \in {1, 2} : Deliver(e, n)
    \/ \E h \in hs : HJit(h) \/ HInsert(h) \/ HDelete(h)

Spec == Init /\ [][Next]_vars

(******************************* Properties ********************************)

\* Quiescent: nothing in flight inside the service, every emitted event handled.
Quiescent ==
    /\ \A e \in emitted : <<e, 1>> \in delivered
    /\ \A h \in hs : h.pc = "done"

\* SAFETY: a VM is never deleted while it is running somebody's job.
NoKilledJob == killed = {}

\* SAFETY: once every job is finished and every event handled, no VM remains.
NoLeakedVM ==
    (Quiescent /\ \A j \in Jobs : jobSt[j] \in {"idle", "done"})
        => \A n \in Names : vm[n] = "absent"

\* SAFETY: once every event is handled, no queued job is left without a
\* runner that could take it.
NoStuckJob ==
    (Quiescent /\ \E j \in Jobs : jobSt[j] = "queued")
        => \E n \in Names : vm[n] = "up" /\ runnerSt[n] = "idle"

=============================================================================
