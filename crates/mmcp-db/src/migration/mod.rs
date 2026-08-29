//! Migration runner.
//!
//! Holds the chronological list of migrations applied to the mmcp
//! database. Newer migrations are appended to the tail of
//! [`Migrator::migrations`].

use sea_orm_migration::prelude::*;

mod m0001_initial;
mod m0002_auth_methods;
mod m0003_indexes;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m0001_initial::Migration),
            Box::new(m0002_auth_methods::Migration),
            Box::new(m0003_indexes::Migration),
        ]
    }
}
