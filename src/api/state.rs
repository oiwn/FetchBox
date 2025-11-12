use std::sync::Arc;

use crate::config::Config;
use crate::handlers::HandlerRegistry;
use crate::ledger::FjallStore;
use crate::observability::Metrics;
use crate::queue::TaskBroker;
use crate::storage::StorageClient;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub registry: Arc<HandlerRegistry>,
    pub store: Arc<FjallStore>,
    pub storage: Arc<StorageClient>,
    pub broker: Arc<TaskBroker>,
    pub metrics: Arc<Metrics>,
}

impl AppState {
    pub fn new(
        config: Config,
        registry: HandlerRegistry,
        store: FjallStore,
        storage: StorageClient,
        broker: Arc<TaskBroker>,
    ) -> Self {
        Self {
            config: Arc::new(config),
            registry: Arc::new(registry),
            store: Arc::new(store),
            storage: Arc::new(storage),
            broker,
            metrics: Arc::new(Metrics::new()),
        }
    }
}

