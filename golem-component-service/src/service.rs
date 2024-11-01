// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_common::config::DbConfig;
use golem_component_service_base::config::ComponentCompilationConfig;
use golem_component_service_base::service::component_compilation::{
    ComponentCompilationService, ComponentCompilationServiceDefault,
    ComponentCompilationServiceDisabled,
};
use golem_service_base::config::{ComponentStoreConfig, IFSStoreConfig};
use golem_service_base::db;
use golem_service_base::service::{component_object_store, ifs_object_store};
use std::sync::Arc;
use crate::config::ComponentServiceConfig;
use golem_component_service_base::repo::component::{
    ComponentRepo, DbComponentRepo, LoggedComponentRepo,
};
use golem_component_service_base::service::component::{ComponentService, ComponentServiceDefault};
use golem_component_service_base::service::ifs::{InitialFileSystemService, InitialFileSystemServiceDefault};
use golem_service_base::auth::DefaultNamespace;

#[derive(Clone)]
pub struct Services {
    pub component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send>,
    pub compilation_service: Arc<dyn ComponentCompilationService + Sync + Send>,
    pub ifs_service: Arc<dyn InitialFileSystemService<DefaultNamespace> + Sync + Send>
}

impl Services {
    pub async fn new(config: &ComponentServiceConfig) -> Result<Services, String> {


        let component_repo: Arc<dyn ComponentRepo + Sync + Send> = match config.db.clone() {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
                    db_pool.clone().into(),
                )))
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                Arc::new(LoggedComponentRepo::new(DbComponentRepo::new(
                    db_pool.clone().into(),
                )))
            }
        };

        let object_store: Arc<dyn component_object_store::ComponentObjectStore + Sync + Send> =
            match config.component_store.clone() {
                ComponentStoreConfig::S3(c) => {
                    Arc::new(component_object_store::AwsS3ComponentObjectStore::new(&c).await)
                }
                ComponentStoreConfig::Local(c) => {
                    Arc::new(component_object_store::FsComponentObjectStore::new(&c)?)
                }
            };

        let ifs_object_store: Arc<dyn ifs_object_store::IFSObjectStore + Sync + Send> =
            match config.ifs_store.clone() {
                IFSStoreConfig::Local(c) => {
                    Arc::new(ifs_object_store::FsIFSObjectStore::new(&c)?)
                }
                IFSStoreConfig::S3(c) => {
                    Arc::new(ifs_object_store::AwsS3IFSObjectStore::new(&c).await)
                }
            };

        let compilation_service: Arc<dyn ComponentCompilationService + Sync + Send> =
            match config.compilation.clone() {
                ComponentCompilationConfig::Enabled(config) => {
                    Arc::new(ComponentCompilationServiceDefault::new(config.uri()))
                }
                ComponentCompilationConfig::Disabled(_) => {
                    Arc::new(ComponentCompilationServiceDisabled)
                }
            };

        let component_service: Arc<dyn ComponentService<DefaultNamespace> + Sync + Send> =
            Arc::new(ComponentServiceDefault::new(
                component_repo.clone(),
                object_store.clone(),
                compilation_service.clone(),
                ifs_object_store.clone()
            ));

        let ifs_service: Arc<dyn InitialFileSystemService<DefaultNamespace> + Sync + Send> =
            Arc::new(InitialFileSystemServiceDefault::new(
                component_repo.clone(),
                ifs_object_store.clone(),
            ));

        Ok(Services {
            component_service,
            compilation_service,
            ifs_service
        })
    }
}
