use std::fmt::{Debug, Formatter};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use async_mutex::Mutex;
use golem_common::cache::PendingOrFinal;
use golem_common::model::{
    AccountId, InvocationKey, VersionedWorkerId, WorkerId, WorkerMetadata, WorkerStatusRecord,
};
use tokio::sync::broadcast::Receiver;
use tracing::{debug, error, info};
use wasmtime::{Store, UpdateDeadline};

use crate::error::GolemError;
use crate::metrics::wasm::{record_create_worker, record_create_worker_failure};
use crate::model::{ExecutionStatus, InterruptKind, WorkerConfig};
use crate::services::golem_config::GolemConfig;
use crate::services::invocation_key::LookupResult;
use crate::services::worker_event::{WorkerEventService, WorkerEventServiceDefault};
use crate::services::{HasAll, HasInvocationKeyService};
use crate::workerctx::WorkerCtx;

pub struct Worker<Ctx: WorkerCtx> {
    pub metadata: WorkerMetadata,
    pub instance: wasmtime::component::Instance,
    pub store: Mutex<Store<Ctx>>,
    pub public_state: Ctx::PublicState,
    pub execution_status: Arc<RwLock<ExecutionStatus>>,
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    pub async fn new<T>(
        this: &T,
        worker_id: WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        template_version: Option<i32>,
        account_id: AccountId,
        pending_worker: &PendingWorker,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx>,
    {
        let start = Instant::now();
        let result = {
            let template_id = worker_id.template_id.clone();

            let (template_version, component) = match template_version {
                Some(component_version) => (
                    component_version,
                    this.template_service()
                        .get(&this.engine(), &template_id, component_version)
                        .await?,
                ),
                None => {
                    this.template_service()
                        .get_latest(&this.engine(), &template_id)
                        .await?
                }
            };

            let versioned_worker_id = VersionedWorkerId {
                worker_id: worker_id.clone(),
                template_version,
            };

            let worker_metadata = WorkerMetadata {
                worker_id: versioned_worker_id.clone(),
                args: worker_args.clone(),
                env: worker_env.clone(),
                account_id,
                last_known_status: WorkerStatusRecord::default(),
            };

            this.worker_service().add(&worker_metadata).await?;

            let execution_status = Arc::new(RwLock::new(ExecutionStatus::Suspended));

            let context = Ctx::create(
                worker_metadata.worker_id.clone(),
                worker_metadata.account_id.clone(),
                this.promise_service().clone(),
                this.invocation_key_service().clone(),
                this.worker_service().clone(),
                this.key_value_service().clone(),
                this.blob_store_service().clone(),
                pending_worker.event_service.clone(),
                this.active_workers().clone(),
                this.extra_deps().clone(),
                this.config(),
                WorkerConfig::new(worker_metadata.worker_id.clone(), worker_args, worker_env),
                execution_status.clone(),
                this.runtime().clone(),
            )
            .await?;

            let public_state = context.get_public_state().clone();

            let mut store = Store::new(&this.engine(), context);
            store.set_epoch_deadline(1);
            store.epoch_deadline_callback(|mut store| {
                let current_level = store.fuel_remaining().unwrap_or(0);
                if store.data().is_out_of_fuel(current_level as i64) {
                    debug!("ran out of fuel, borrowing more");
                    store.data_mut().borrow_fuel_sync();
                }

                match store.data_mut().check_interrupt() {
                    Some(kind) => Err(kind.into()),
                    None => Ok(UpdateDeadline::Yield(1)),
                }
            });

            store.add_fuel(i64::MAX as u64)?;
            store.data_mut().borrow_fuel().await?; // Borrowing fuel for initialization and also to make sure account is in cache

            store.limiter_async(|ctx| ctx.resource_limiter());

            let instance_pre = this.linker().instantiate_pre(&component).map_err(|e| {
                GolemError::worker_creation_failed(
                    worker_id.clone(),
                    format!("Failed to pre-instantiate component: {e}"),
                )
            })?;

            let instance = instance_pre
                .instantiate_async(&mut store)
                .await
                .map_err(|e| {
                    GolemError::worker_creation_failed(
                        worker_id.clone(),
                        format!("Failed to instantiate component: {e}"),
                    )
                })?;

            Ctx::prepare_instance(&versioned_worker_id, &instance, &mut store).await?;

            let result = Arc::new(Worker {
                metadata: worker_metadata.clone(),
                instance,
                store: Mutex::new(store),
                public_state,
                execution_status,
            });

            info!("Worker {}/{} activated", worker_id.slug(), template_version);

            Ok(result)
        };

        match &result {
            Ok(_) => record_create_worker(start.elapsed()),
            Err(err) => record_create_worker_failure(err),
        }

        result
    }

    /// Makes sure that the worker is active, but without waiting for it to be idle.
    ///
    /// If the worker is already in memory this does nothing. Otherwise the worker will be
    /// created (same as get_or_create_worker) but in a background task.
    ///
    /// If the active worker cache is not full, this newly created worker will be added to it.
    /// If it was full, the worker will be dropped but only after it finishes recovering which means
    /// a previously interrupted / suspended invocation might be resumed.
    pub async fn activate<T>(
        this: &T,
        worker_id: WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        template_version: Option<i32>,
        account_id: AccountId,
    ) where
        T: HasAll<Ctx> + Send + Sync + Clone + 'static,
    {
        let worker_id_clone = worker_id.clone();
        let this_clone = this.clone();
        tokio::task::spawn(async move {
            let result = Worker::get_or_create(
                &this_clone,
                worker_id,
                worker_args,
                worker_env,
                template_version,
                account_id,
            )
            .await;
            if let Err(err) = result {
                error!("Failed to activate worker {worker_id_clone}: {err}");
            }
        });
    }

    pub async fn get_or_create<T>(
        this: &T,
        worker_id: WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        template_version: Option<i32>,
        account_id: AccountId,
    ) -> Result<Arc<Self>, GolemError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let this_clone = this.clone();
        let worker_id_clone = worker_id.clone();
        let worker_args_clone = worker_args.clone();
        let worker_env_clone = worker_env.clone();
        let config_clone = this.config().clone();
        let worker_details = this
            .active_workers()
            .get_with(
                worker_id.clone(),
                || PendingWorker::new(config_clone),
                |pending_worker| {
                    let pending_worker_clone = pending_worker.clone();
                    Box::pin(async move {
                        Worker::new(
                            &this_clone,
                            worker_id_clone,
                            worker_args_clone,
                            worker_env_clone,
                            template_version,
                            account_id,
                            &pending_worker_clone,
                        )
                        .await
                    })
                },
            )
            .await?;
        validate_worker(
            worker_details.metadata.clone(),
            worker_args,
            worker_env,
            template_version,
        )?;
        Ok(worker_details)
    }

    pub async fn get_or_create_pending<T>(
        this: &T,
        worker_id: WorkerId,
        worker_args: Vec<String>,
        worker_env: Vec<(String, String)>,
        template_version: Option<i32>,
        account_id: AccountId,
    ) -> Result<PendingOrFinal<PendingWorker, Arc<Self>>, GolemError>
    where
        T: HasAll<Ctx> + Clone + Send + Sync + 'static,
    {
        let this_clone = this.clone();
        let worker_id_clone = worker_id.clone();
        let worker_args_clone = worker_args.clone();
        let worker_env_clone = worker_env.clone();
        let config_clone = this.config().clone();
        this.active_workers()
            .get_pending_with(
                worker_id.clone(),
                || PendingWorker::new(config_clone),
                move |pending_worker| {
                    let pending_worker_clone = pending_worker.clone();
                    Box::pin(async move {
                        Worker::new(
                            &this_clone,
                            worker_id_clone,
                            worker_args_clone,
                            worker_env_clone,
                            template_version,
                            account_id,
                            &pending_worker_clone,
                        )
                        .await
                    })
                },
            )
            .await
    }

    /// Looks up a given invocation key's current status.
    /// As the invocation key status is only stored in memory, we need to have an active
    /// instance (instance_details) to call this function.
    pub fn lookup_result<T>(&self, this: &T, invocation_key: &InvocationKey) -> LookupResult
    where
        T: HasInvocationKeyService,
    {
        this.invocation_key_service()
            .lookup_key(&self.metadata.worker_id.worker_id, invocation_key)
    }

    pub fn set_interrupting(&self, interrupt_kind: InterruptKind) -> Option<Receiver<()>> {
        let mut execution_status = self.execution_status.write().unwrap();
        let current_execution_status = execution_status.clone();
        match current_execution_status {
            ExecutionStatus::Running => {
                let (sender, receiver) = tokio::sync::broadcast::channel(1);
                *execution_status = ExecutionStatus::Interrupting {
                    interrupt_kind,
                    await_interruption: Arc::new(sender),
                };
                Some(receiver)
            }
            ExecutionStatus::Suspended => {
                *execution_status = ExecutionStatus::Interrupted { interrupt_kind };
                None
            }
            ExecutionStatus::Interrupting {
                await_interruption, ..
            } => {
                let receiver = await_interruption.subscribe();
                Some(receiver)
            }
            ExecutionStatus::Interrupted { .. } => None,
        }
    }
}

impl<Ctx: WorkerCtx> Drop for Worker<Ctx> {
    fn drop(&mut self) {
        info!("Deactivated worker {}", self.metadata.worker_id);
    }
}

impl<Ctx: WorkerCtx> Debug for Worker<Ctx> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "WorkerDetails({})", self.metadata.worker_id)
    }
}

#[derive(Clone)]
pub struct PendingWorker {
    pub event_service: Arc<dyn WorkerEventService + Send + Sync>,
}

impl PendingWorker {
    pub fn new(config: Arc<GolemConfig>) -> Result<PendingWorker, GolemError> {
        Ok(PendingWorker {
            event_service: Arc::new(WorkerEventServiceDefault::new(
                config.limits.event_broadcast_capacity,
                config.limits.event_history_size,
            )),
        })
    }
}

fn validate_worker(
    worker_metadata: WorkerMetadata,
    worker_args: Vec<String>,
    worker_env: Vec<(String, String)>,
    template_version: Option<i32>,
) -> Result<(), GolemError> {
    let mut errors: Vec<String> = Vec::new();
    if worker_metadata.args != worker_args {
        let error = format!(
            "Worker is already running with different args: {:?} != {:?}",
            worker_metadata.args, worker_args
        );
        errors.push(error)
    }
    if worker_metadata.env != worker_env {
        let error = format!(
            "Worker is already running with different env: {:?} != {:?}",
            worker_metadata.env, worker_env
        );
        errors.push(error)
    }
    if let Some(version) = template_version {
        if worker_metadata.worker_id.template_version != version {
            let error = format!(
                "Worker is already running with different template version: {:?} != {:?}",
                worker_metadata.worker_id.template_version, version
            );
            errors.push(error)
        }
    };
    if errors.is_empty() {
        Ok(())
    } else {
        Err(GolemError::worker_creation_failed(
            worker_metadata.worker_id.worker_id,
            errors.join("\n"),
        ))
    }
}