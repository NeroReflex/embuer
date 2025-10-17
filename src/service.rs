use crate::{config::Config, ServiceError};

pub struct Service {
    config: Config,
}

impl Service {
    pub fn new(config: Config) -> Result<Self, ServiceError> {
        Ok(Self { config })
    }
}
