use std::sync::Arc;

use tokio::sync::RwLock;
use zbus::interface;

use crate::{service::Service, ServiceError};

pub struct EmbuerDBus {
    service: Arc<RwLock<Service>>,
}

impl EmbuerDBus {
    pub fn new(service: Arc<RwLock<Service>>) -> Self {
        Self { service }
    }
}

#[interface(
    name = "org.neroreflex.embuer1",
    proxy(
        default_service = "org.neroreflex.embuer",
        default_path = "/org/neroreflex/login_ng_service"
    )
)]
impl EmbuerDBus {}
