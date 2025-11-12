pub mod api;
pub mod config;
pub mod handlers;
pub mod humanize;
pub mod ledger;
pub mod messaging;  // Expose for tests (MockProducer)
pub mod observability;
pub mod proto;
pub mod queue;
pub mod storage;

// Disable worker module for now during API isolation
// pub mod worker;
