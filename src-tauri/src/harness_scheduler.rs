//! Deterministic bounded scheduler for model tool calls.
//!
//! Read-only calls may share a rolling pool. Mutations are barriers: nothing
//! after a mutation can pass it, and it starts only after all earlier reads
//! finish. Cancellation synthesizes a terminal result for every undispatched
//! call so the model never observes an unpaired call.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyClass {
    ConcurrentRead,
    ExclusiveMutation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledCall {
    pub call_id: String,
    pub class: ConcurrencyClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalOutcome {
    Completed,
    Failed,
    AbortedBeforeDispatch,
    CancelledWhileRunning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalCall {
    pub call_id: String,
    pub outcome: TerminalOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerError {
    InvalidLimit,
    DuplicateCall(String),
    UnknownCall(String),
    DuplicateTerminal(String),
    Cancelled,
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimit => write!(f, "scheduler concurrency limit must be positive"),
            Self::DuplicateCall(call) => write!(f, "duplicate scheduled call {call}"),
            Self::UnknownCall(call) => write!(f, "unknown running call {call}"),
            Self::DuplicateTerminal(call) => write!(f, "duplicate terminal call {call}"),
            Self::Cancelled => write!(f, "scheduler is cancelled"),
        }
    }
}

impl std::error::Error for SchedulerError {}

#[derive(Debug)]
pub struct RollingScheduler {
    max_concurrent_reads: usize,
    pending: VecDeque<ScheduledCall>,
    running: BTreeMap<String, ConcurrencyClass>,
    known: BTreeSet<String>,
    terminal: BTreeMap<String, TerminalOutcome>,
    cancelled: bool,
}

impl RollingScheduler {
    pub fn new(max_concurrent_reads: usize) -> Result<Self, SchedulerError> {
        if max_concurrent_reads == 0 {
            return Err(SchedulerError::InvalidLimit);
        }
        Ok(Self {
            max_concurrent_reads,
            pending: VecDeque::new(),
            running: BTreeMap::new(),
            known: BTreeSet::new(),
            terminal: BTreeMap::new(),
            cancelled: false,
        })
    }

    pub fn submit(&mut self, call: ScheduledCall) -> Result<(), SchedulerError> {
        if self.cancelled {
            return Err(SchedulerError::Cancelled);
        }
        if !self.known.insert(call.call_id.clone()) {
            return Err(SchedulerError::DuplicateCall(call.call_id));
        }
        self.pending.push_back(call);
        Ok(())
    }

    pub fn dispatch_ready(&mut self) -> Vec<ScheduledCall> {
        if self.cancelled
            || self
                .running
                .values()
                .any(|class| *class == ConcurrencyClass::ExclusiveMutation)
        {
            return Vec::new();
        }
        let Some(front) = self.pending.front() else {
            return Vec::new();
        };
        if front.class == ConcurrencyClass::ExclusiveMutation {
            if self.running.is_empty() {
                let call = self.pending.pop_front().expect("front exists");
                self.running.insert(call.call_id.clone(), call.class);
                return vec![call];
            }
            return Vec::new();
        }

        let mut dispatched = Vec::new();
        while self.running.len() < self.max_concurrent_reads {
            let Some(front) = self.pending.front() else {
                break;
            };
            if front.class == ConcurrencyClass::ExclusiveMutation {
                break;
            }
            let call = self.pending.pop_front().expect("front exists");
            self.running.insert(call.call_id.clone(), call.class);
            dispatched.push(call);
        }
        dispatched
    }

    pub fn complete(
        &mut self,
        call_id: &str,
        outcome: TerminalOutcome,
    ) -> Result<TerminalCall, SchedulerError> {
        if self.terminal.contains_key(call_id) {
            return Err(SchedulerError::DuplicateTerminal(call_id.to_string()));
        }
        if self.running.remove(call_id).is_none() {
            return Err(SchedulerError::UnknownCall(call_id.to_string()));
        }
        self.terminal.insert(call_id.to_string(), outcome);
        Ok(TerminalCall {
            call_id: call_id.to_string(),
            outcome,
        })
    }

    /// Stop future dispatch and synthesize exactly one terminal result for each
    /// call that never reached the tool body. Running calls remain owned by the
    /// executor and must converge through `complete`.
    pub fn cancel(&mut self) -> Vec<TerminalCall> {
        self.cancelled = true;
        let mut terminals = Vec::new();
        while let Some(call) = self.pending.pop_front() {
            if self.terminal.contains_key(&call.call_id) {
                continue;
            }
            self.terminal
                .insert(call.call_id.clone(), TerminalOutcome::AbortedBeforeDispatch);
            terminals.push(TerminalCall {
                call_id: call.call_id,
                outcome: TerminalOutcome::AbortedBeforeDispatch,
            });
        }
        terminals
    }

    pub fn running_ids(&self) -> BTreeSet<String> {
        self.running.keys().cloned().collect()
    }

    pub fn terminal_count(&self) -> usize {
        self.terminal.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str, class: ConcurrencyClass) -> ScheduledCall {
        ScheduledCall {
            call_id: id.into(),
            class,
        }
    }

    #[test]
    fn reads_roll_in_parallel_but_mutation_is_a_strict_barrier() {
        let mut scheduler = RollingScheduler::new(2).unwrap();
        scheduler
            .submit(call("r1", ConcurrencyClass::ConcurrentRead))
            .unwrap();
        scheduler
            .submit(call("r2", ConcurrencyClass::ConcurrentRead))
            .unwrap();
        scheduler
            .submit(call("w1", ConcurrencyClass::ExclusiveMutation))
            .unwrap();
        scheduler
            .submit(call("r3", ConcurrencyClass::ConcurrentRead))
            .unwrap();

        assert_eq!(
            scheduler
                .dispatch_ready()
                .into_iter()
                .map(|call| call.call_id)
                .collect::<Vec<_>>(),
            ["r1", "r2"]
        );
        scheduler
            .complete("r1", TerminalOutcome::Completed)
            .unwrap();
        assert!(scheduler.dispatch_ready().is_empty(), "w1 blocks later r3");
        scheduler
            .complete("r2", TerminalOutcome::Completed)
            .unwrap();
        assert_eq!(scheduler.dispatch_ready()[0].call_id, "w1");
        assert!(scheduler.dispatch_ready().is_empty());
        scheduler
            .complete("w1", TerminalOutcome::Completed)
            .unwrap();
        assert_eq!(scheduler.dispatch_ready()[0].call_id, "r3");
    }

    #[test]
    fn cancellation_pairs_every_undispatched_call_once() {
        let mut scheduler = RollingScheduler::new(1).unwrap();
        for id in ["r1", "r2", "w1"] {
            scheduler
                .submit(call(
                    id,
                    if id == "w1" {
                        ConcurrencyClass::ExclusiveMutation
                    } else {
                        ConcurrencyClass::ConcurrentRead
                    },
                ))
                .unwrap();
        }
        assert_eq!(scheduler.dispatch_ready()[0].call_id, "r1");
        let aborted = scheduler.cancel();
        assert_eq!(
            aborted
                .iter()
                .map(|call| call.call_id.as_str())
                .collect::<Vec<_>>(),
            ["r2", "w1"]
        );
        assert!(scheduler.cancel().is_empty());
        scheduler
            .complete("r1", TerminalOutcome::CancelledWhileRunning)
            .unwrap();
        assert_eq!(scheduler.terminal_count(), 3);
    }

    #[test]
    fn duplicate_ids_and_terminal_results_are_rejected() {
        let mut scheduler = RollingScheduler::new(1).unwrap();
        scheduler
            .submit(call("r1", ConcurrencyClass::ConcurrentRead))
            .unwrap();
        assert!(matches!(
            scheduler.submit(call("r1", ConcurrencyClass::ConcurrentRead)),
            Err(SchedulerError::DuplicateCall(_))
        ));
        scheduler.dispatch_ready();
        scheduler
            .complete("r1", TerminalOutcome::Completed)
            .unwrap();
        assert!(matches!(
            scheduler.complete("r1", TerminalOutcome::Completed),
            Err(SchedulerError::DuplicateTerminal(_))
        ));
    }

    #[test]
    fn delayed_reads_respect_rolling_window_limit_of_four() {
        let mut scheduler = RollingScheduler::new(4).unwrap();
        for ordinal in 0..8 {
            scheduler
                .submit(call(
                    &format!("read-{ordinal}"),
                    ConcurrencyClass::ConcurrentRead,
                ))
                .unwrap();
        }
        assert_eq!(scheduler.dispatch_ready().len(), 4);
        assert!(scheduler.dispatch_ready().is_empty());
        scheduler
            .complete("read-0", TerminalOutcome::Completed)
            .unwrap();
        assert_eq!(scheduler.dispatch_ready()[0].call_id, "read-4");
    }

    #[test]
    fn mutation_barrier_is_globally_exclusive_with_interleaved_reads() {
        let mut scheduler = RollingScheduler::new(4).unwrap();
        for (id, class) in [
            ("r1", ConcurrencyClass::ConcurrentRead),
            ("r2", ConcurrencyClass::ConcurrentRead),
            ("m1", ConcurrencyClass::ExclusiveMutation),
            ("r3", ConcurrencyClass::ConcurrentRead),
        ] {
            scheduler.submit(call(id, class)).unwrap();
        }
        assert_eq!(scheduler.dispatch_ready().len(), 2);
        scheduler
            .complete("r1", TerminalOutcome::Completed)
            .unwrap();
        assert!(scheduler.dispatch_ready().is_empty());
        scheduler
            .complete("r2", TerminalOutcome::Completed)
            .unwrap();
        let mutation = scheduler.dispatch_ready();
        assert_eq!(mutation[0].call_id, "m1");
        assert_eq!(scheduler.running_ids(), BTreeSet::from(["m1".to_string()]));
        scheduler
            .complete("m1", TerminalOutcome::Completed)
            .unwrap();
        assert_eq!(scheduler.dispatch_ready()[0].call_id, "r3");
    }

    #[test]
    fn cancel_gives_exactly_one_terminal_per_undispatched_call() {
        let mut scheduler = RollingScheduler::new(2).unwrap();
        for ordinal in 0..6 {
            scheduler
                .submit(call(
                    &format!("c-{ordinal}"),
                    if ordinal == 3 {
                        ConcurrencyClass::ExclusiveMutation
                    } else {
                        ConcurrencyClass::ConcurrentRead
                    },
                ))
                .unwrap();
        }
        assert_eq!(scheduler.dispatch_ready().len(), 2);
        let aborted = scheduler.cancel();
        assert_eq!(aborted.len(), 4);
        assert!(aborted
            .iter()
            .all(|call| call.outcome == TerminalOutcome::AbortedBeforeDispatch));
        assert!(scheduler.cancel().is_empty());
        scheduler
            .complete("c-0", TerminalOutcome::CancelledWhileRunning)
            .unwrap();
        scheduler
            .complete("c-1", TerminalOutcome::CancelledWhileRunning)
            .unwrap();
        assert_eq!(scheduler.terminal_count(), 6);
    }

    #[test]
    fn hundred_call_competition_preserves_mutation_exclusivity_and_pairs_all_results() {
        let mut scheduler = RollingScheduler::new(4).unwrap();
        for ordinal in 0..100 {
            scheduler
                .submit(call(
                    &format!("call-{ordinal:03}"),
                    if ordinal % 10 == 5 {
                        ConcurrencyClass::ExclusiveMutation
                    } else {
                        ConcurrencyClass::ConcurrentRead
                    },
                ))
                .unwrap();
        }

        while scheduler.terminal_count() < 100 {
            let ready = scheduler.dispatch_ready();
            assert!(
                !ready.is_empty(),
                "non-cancelled scheduler must make progress"
            );
            if ready
                .iter()
                .any(|call| call.class == ConcurrencyClass::ExclusiveMutation)
            {
                assert_eq!(ready.len(), 1, "mutation dispatch is globally exclusive");
                assert_eq!(scheduler.running_ids().len(), 1);
            } else {
                assert!(ready.len() <= 4);
                assert!(scheduler.running_ids().len() <= 4);
            }
            for call in ready {
                scheduler
                    .complete(&call.call_id, TerminalOutcome::Completed)
                    .unwrap();
            }
        }
        assert!(scheduler.running_ids().is_empty());
        assert_eq!(scheduler.terminal_count(), 100);
    }
}
