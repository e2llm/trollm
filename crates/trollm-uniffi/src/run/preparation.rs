//! Run-owned preparation. No inference is performed here. Native loading is
//! synchronous: stopping a run joins any load already in progress.
#[cfg(any(feature = "llama", test))]
use super::*;

#[cfg(not(any(feature = "llama", test)))]
#[derive(Default)]
pub(super) struct Preparations {}

#[cfg(not(any(feature = "llama", test)))]
impl Preparations {
    pub(super) fn start(&mut self, _: std::time::Duration) -> anyhow::Result<()> {
        Ok(())
    }
    pub(super) fn stop_and_join(&mut self) {}
}

#[cfg(any(feature = "llama", test))]
mod enabled {
    use super::*;
    use std::thread::JoinHandle;
    use trollm_models::GenerationDiagnostics;

    /// Only slots used by authored tasks are eligible. Plans using more than
    /// two distinct slots retain the lazy path, so a third lazy load cannot
    /// overlap two preparers. Ordering makes coder the first scheduled job.
    pub(in super::super) fn selected_slots(
        plan: &ExecutionPlan,
        registry: &TaskRegistry,
    ) -> Vec<String> {
        fn collect(plan: &ExecutionPlan, registry: &TaskRegistry, slots: &mut Vec<String>) {
            for node in plan.nodes.values() {
                if let Some(task) = node
                    .task_ref
                    .as_ref()
                    .and_then(|key| registry.tasks.get(key))
                {
                    if !slots.contains(&task.model) {
                        slots.push(task.model.clone());
                    }
                }
            }
            for child in plan.subflows.values() {
                collect(child, registry, slots);
            }
        }
        let mut slots = Vec::new();
        collect(plan, registry, &mut slots);
        slots.sort_by(|a, b| (a != "coder", a).cmp(&(b != "coder", b)));
        if slots.len() > 2 {
            return Vec::new();
        }
        slots
    }

    struct Completion {
        result: Option<Result<Box<dyn ModelSlot>, String>>,
        diagnostics: Option<GenerationDiagnostics>,
    }

    struct Shared {
        state: Mutex<Completion>,
        ready: Condvar,
        ctl: Arc<RunControl>,
        slot: String,
    }

    struct Job {
        lazy: LazyModel,
        shared: Arc<Shared>,
    }

    #[derive(Default)]
    pub(in super::super) struct Preparations {
        pending: Vec<Job>,
        workers: Vec<JoinHandle<()>>,
        controls: Vec<Arc<RunControl>>,
        stop: Arc<AtomicBool>,
    }

    impl Preparations {
        pub(in super::super) fn add(&mut self, lazy: LazyModel) -> Box<dyn ModelSlot> {
            assert!(
                self.controls.len() < 2,
                "preparation is bounded to two slots"
            );
            let shared = Arc::new(Shared {
                state: Mutex::new(Completion {
                    result: None,
                    diagnostics: None,
                }),
                ready: Condvar::new(),
                ctl: lazy.ctl.clone(),
                slot: lazy.slot.clone(),
            });
            self.controls.push(lazy.ctl.clone());
            self.pending.push(Job {
                lazy,
                shared: shared.clone(),
            });
            Box::new(PreparedModel {
                shared,
                loaded: None,
                failure: None,
            })
        }

        pub(in super::super) fn start(&mut self, remaining: Duration) -> Result<()> {
            let deadline = Instant::now().checked_add(remaining);
            // Scheduling priority, not an inference priority or a third worker.
            self.pending.sort_by_key(|job| job.lazy.slot != "coder");
            for job in self.pending.drain(..) {
                let stop = self.stop.clone();
                self.workers.push(
                    std::thread::Builder::new()
                        .name(format!("trollm-prepare-{}", job.lazy.slot))
                        .spawn(move || {
                            let mut diagnostics = GenerationDiagnostics {
                                preparation_async: Some(true),
                                ..Default::default()
                            };
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    prepare(job.lazy, &stop, deadline, &mut diagnostics)
                                }))
                                .unwrap_or_else(|_| {
                                    Err(anyhow!("model preparation worker panicked"))
                                })
                                .map_err(|error| format!("{error:#}"));
                            // Each worker removes only its own activity, including on panic.
                            job.shared
                                .ctl
                                .preparation_activity
                                .lock()
                                .unwrap()
                                .remove(&job.shared.slot);
                            let mut state = job.shared.state.lock().unwrap();
                            state.diagnostics = Some(diagnostics);
                            state.result = Some(result);
                            job.shared.ready.notify_all();
                        })
                        .context("spawning model preparation worker")?,
                );
            }
            Ok(())
        }
    }

    impl Preparations {
        #[cfg(test)]
        pub(in super::super) fn finish_for_test(&mut self) {
            for worker in self.workers.drain(..) {
                worker.join().unwrap();
            }
        }

        pub(in super::super) fn stop_and_join(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // Synchronize with the background checkpoint's check-to-wait window.
            for ctl in &self.controls {
                let _gate = ctl.gate.lock().unwrap_or_else(|error| error.into_inner());
                ctl.gate_cond.notify_all();
            }
            for worker in self.workers.drain(..) {
                let _ = worker.join();
            }
        }
    }

    impl Drop for Preparations {
        fn drop(&mut self) {
            self.stop_and_join();
        }
    }

    /// Background work never changes the foreground phase or counts holds.
    fn checkpoint(ctl: &RunControl, stop: &AtomicBool, deadline: Option<Instant>) -> Result<()> {
        let mut gate = ctl
            .gate
            .lock()
            .map_err(|_| anyhow!("run control lock poisoned"))?;
        loop {
            if stop.load(Ordering::SeqCst) || ctl.is_cancelled() {
                bail!(CANCEL_MARKER);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(GenerationExpired.into());
            }
            if !ctl.pause_requested.load(Ordering::SeqCst) && !gate.plan_paused {
                return Ok(());
            }
            let timeout = deadline.map_or(Duration::from_millis(50), |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50))
            });
            gate = ctl
                .gate_cond
                .wait_timeout(gate, timeout)
                .map_err(|_| anyhow!("run control lock poisoned"))?
                .0;
        }
    }

    fn prepare(
        mut lazy: LazyModel,
        stop: &AtomicBool,
        deadline: Option<Instant>,
        diagnostics: &mut GenerationDiagnostics,
    ) -> Result<Box<dyn ModelSlot>> {
        let check = || checkpoint(&lazy.ctl, stop, deadline);
        check()?;
        lazy.ctl.preparation_activity.lock().unwrap().insert(
            lazy.slot.clone(),
            RunActivityApi::VerifyingModel {
                slot: lazy.slot.clone(),
            },
        );
        let started = Instant::now();
        let verification = (|| {
            let (path, expected) = match &lazy.weights {
                Some(weights) => weights.clone(),
                None => {
                    let path = lazy.base.join("models").join(format!("{}.gguf", lazy.slot));
                    let manifest = fs::read_to_string(path.with_extension("gguf.sha256"))
                        .with_context(|| {
                            format!("no sha256 manifest for model slot '{}'", lazy.slot)
                        })?;
                    (
                        path,
                        manifest.split_whitespace().next().unwrap_or("").to_owned(),
                    )
                }
            };
            verify_weights_with_checkpoint(&path, &expected, &lazy.slot, &check)?;
            Ok::<_, anyhow::Error>(path)
        })();
        diagnostics.weight_verify_ms = Some(started.elapsed().as_millis() as u64);
        let path = verification?;
        check()?;
        lazy.ctl.preparation_activity.lock().unwrap().insert(
            lazy.slot.clone(),
            RunActivityApi::LoadingModel {
                slot: lazy.slot.clone(),
            },
        );
        let started = Instant::now();
        let loaded = (lazy.loader)(&path);
        diagnostics.load_ms = Some(started.elapsed().as_millis() as u64);
        // Loading cannot be interrupted, but its result must not escape expired
        // or cancelled preparation. Drop joins this call before run inactivity.
        check()?;
        loaded
    }

    struct PreparedModel {
        shared: Arc<Shared>,
        loaded: Option<Box<dyn ModelSlot>>,
        failure: Option<String>,
    }

    impl ModelSlot for PreparedModel {
        fn generate(&mut self, mut req: GenerateRequest) -> Result<GenerateResponse> {
            let budget = GenerationBudget::new(req.max_duration);
            let waiting = Instant::now();
            let was_waiting = self.loaded.is_none() && self.failure.is_none();
            let mut diagnostics = GenerationDiagnostics::default();
            let result = (|| {
                loop {
                    if was_waiting {
                        // Replace ControlledModel's provisional activity with this
                        // slot's actual preparation phase. Background workers never
                        // write foreground activity themselves.
                        let activity = self
                            .shared
                            .ctl
                            .preparation_activity
                            .lock()
                            .unwrap()
                            .get(&self.shared.slot)
                            .cloned();
                        self.shared.ctl.set_activity(activity);
                    }
                    self.shared.ctl.before_boundary()?;
                    if budget.expired() {
                        return Ok(generation_timeout());
                    }
                    if self.loaded.is_some() || self.failure.is_some() {
                        break;
                    }
                    let mut state = self.shared.state.lock().unwrap();
                    if let Some(result) = state.result.take() {
                        diagnostics = state.diagnostics.take().unwrap_or_default();
                        diagnostics.preparation_wait_ms =
                            Some(waiting.elapsed().as_millis() as u64);
                        match result {
                            Ok(model) => self.loaded = Some(model),
                            Err(error) => self.failure = Some(error),
                        }
                        break;
                    }
                    let timeout = budget
                        .remaining()
                        .unwrap_or(Duration::from_millis(25))
                        .min(Duration::from_millis(25));
                    drop(self.shared.ready.wait_timeout(state, timeout).unwrap());
                }
                if let Some(error) = &self.failure {
                    bail!("{error}");
                }
                self.shared.ctl.before_boundary()?;
                if budget.expired() {
                    return Ok(generation_timeout());
                }
                self.shared
                    .ctl
                    .set_activity(Some(RunActivityApi::Generating {
                        slot: self.shared.slot.clone(),
                    }));
                if was_waiting {
                    diagnostics.preparation_wait_ms = Some(waiting.elapsed().as_millis() as u64);
                }
                req.max_duration = budget.remaining();
                self.loaded.as_mut().unwrap().generate(req)
            })();
            if was_waiting {
                diagnostics.preparation_async = Some(true);
                diagnostics
                    .preparation_wait_ms
                    .get_or_insert_with(|| waiting.elapsed().as_millis() as u64);
            }
            match result {
                Ok(mut response) => {
                    response
                        .diagnostics
                        .get_or_insert_default()
                        .include_loading(&diagnostics);
                    Ok(response)
                }
                Err(error) => {
                    let mut combined = error
                        .downcast_ref::<trollm_models::ModelDiagnosticError>()
                        .map(|error| error.diagnostics.clone())
                        .unwrap_or_default();
                    combined.include_loading(&diagnostics);
                    Err(trollm_models::ModelDiagnosticError::attach(error, combined))
                }
            }
        }
    }
}

#[cfg(any(feature = "llama", test))]
pub(super) use enabled::selected_slots;
#[cfg(any(feature = "llama", test))]
pub(super) use enabled::Preparations;
